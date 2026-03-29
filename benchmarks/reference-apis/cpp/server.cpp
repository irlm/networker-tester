// server.cpp — C++ Boost.Beast + Boost.Asio SSL reference API for AletheBench.
//
// Endpoints:
//   GET  /health          → {"status":"ok","runtime":"cpp","version":"<__cplusplus>"}
//   GET  /download/{size} → stream `size` bytes of 0x42 in 8192-byte chunks
//   POST /upload          → consume body, return {"bytes_received": N}
//
// Listens on port 8443 (or BENCH_PORT) with TLS.

#include <boost/asio.hpp>
#include <boost/asio/ssl.hpp>
#include <boost/beast.hpp>
#include <boost/beast/ssl.hpp>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <string>
#include <thread>

namespace beast = boost::beast;
namespace http  = beast::http;
namespace net   = boost::asio;
namespace ssl   = net::ssl;
using tcp       = net::ip::tcp;

// ── Helpers ─────────────────────────────────────────────────────────────────

static std::string cplusplus_version() {
    return std::to_string(__cplusplus);
}

static std::string json_health() {
    return R"({"status":"ok","runtime":"cpp","version":")" + cplusplus_version() + R"("})";
}

static std::string json_bytes_received(std::uint64_t n) {
    return R"({"bytes_received":)" + std::to_string(n) + "}";
}

// Parse a path like "/download/1048576" and extract the size.
// Returns 0 on failure.
static std::uint64_t parse_download_size(beast::string_view target) {
    constexpr beast::string_view prefix = "/download/";
    if (target.size() <= prefix.size())
        return 0;
    if (target.substr(0, prefix.size()) != prefix)
        return 0;
    auto num_str = target.substr(prefix.size());
    // Strip query string if present
    auto qpos = num_str.find('?');
    if (qpos != beast::string_view::npos)
        num_str = num_str.substr(0, qpos);
    try {
        return std::stoull(std::string(num_str));
    } catch (...) {
        return 0;
    }
}

// ── Request handler ─────────────────────────────────────────────────────────

template <class Send>
void handle_request(http::request<http::string_body>&& req, Send&& send) {
    auto const bad_request = [&req](beast::string_view why) {
        http::response<http::string_body> res{http::status::bad_request, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = R"({"error":")" + std::string(why) + R"("})";
        res.prepare_payload();
        return res;
    };

    auto const not_found = [&req]() {
        http::response<http::string_body> res{http::status::not_found, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = R"({"error":"not found"})";
        res.prepare_payload();
        return res;
    };

    auto target = req.target();

    // GET /health
    if (req.method() == http::verb::get && target == "/health") {
        http::response<http::string_body> res{http::status::ok, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = json_health();
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /download/{size}
    if (req.method() == http::verb::get && target.starts_with("/download/")) {
        auto size = parse_download_size(target);
        if (size == 0) {
            return send(bad_request("invalid size"));
        }

        // Use a chunked / streaming approach via a body generator.
        // Beast doesn't have a built-in streaming body, so we use a buffer_body
        // and write in a loop. However, for the async send interface we use a
        // simple string body with the full payload for smaller sizes, or a
        // custom approach for large sizes.
        //
        // For benchmark accuracy, we use http::response_serializer with
        // http::buffer_body to stream chunks without buffering the whole payload.

        http::response<http::buffer_body> res;
        res.result(http::status::ok);
        res.version(req.version());
        res.set(http::field::content_type, "application/octet-stream");
        res.content_length(size);
        res.keep_alive(req.keep_alive());

        // Signal "more data coming"
        res.body().data = nullptr;
        res.body().size = 0;
        res.body().more = true;

        return send(std::move(res), size);
    }

    // POST /upload
    if (req.method() == http::verb::post && target == "/upload") {
        auto bytes = req.body().size();
        http::response<http::string_body> res{http::status::ok, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = json_bytes_received(bytes);
        res.prepare_payload();
        return send(std::move(res));
    }

    return send(not_found());
}

// ── Session ─────────────────────────────────────────────────────────────────

class session : public std::enable_shared_from_this<session> {
    beast::ssl_stream<beast::tcp_stream> stream_;
    beast::flat_buffer buffer_;
    http::request<http::string_body> req_;

public:
    explicit session(tcp::socket&& socket, ssl::context& ctx)
        : stream_(std::move(socket), ctx) {}

    void run() {
        net::dispatch(
            stream_.get_executor(),
            beast::bind_front_handler(&session::on_run, shared_from_this()));
    }

private:
    void on_run() {
        beast::get_lowest_layer(stream_).expires_after(std::chrono::seconds(30));
        stream_.async_handshake(
            ssl::stream_base::server,
            beast::bind_front_handler(&session::on_handshake, shared_from_this()));
    }

    void on_handshake(beast::error_code ec) {
        if (ec) return;
        do_read();
    }

    void do_read() {
        req_ = {};
        beast::get_lowest_layer(stream_).expires_after(std::chrono::seconds(30));
        http::async_read(
            stream_, buffer_, req_,
            beast::bind_front_handler(&session::on_read, shared_from_this()));
    }

    void on_read(beast::error_code ec, std::size_t /*bytes_transferred*/) {
        if (ec == http::error::end_of_stream)
            return do_close();
        if (ec) return;

        handle_request(std::move(req_), [this](auto&& msg, std::uint64_t download_size = 0) {
            using msg_type = std::decay_t<decltype(msg)>;

            if constexpr (std::is_same_v<msg_type, http::response<http::buffer_body>>) {
                // Streaming download response
                do_write_download(std::move(msg), download_size);
            } else {
                // Normal response — store in shared_ptr for lifetime
                auto sp = std::make_shared<msg_type>(std::move(msg));
                http::async_write(
                    stream_, *sp,
                    [self = shared_from_this(), sp](beast::error_code ec, std::size_t) {
                        if (ec) return;
                        if (sp->need_eof())
                            return self->do_close();
                        self->do_read();
                    });
            }
        });
    }

    void do_write_download(http::response<http::buffer_body> header, std::uint64_t total) {
        auto hdr = std::make_shared<http::response<http::buffer_body>>(std::move(header));
        auto sr  = std::make_shared<http::response_serializer<http::buffer_body>>(*hdr);

        // Write the header first
        http::async_write_header(
            stream_, *sr,
            [self = shared_from_this(), hdr, sr, total](beast::error_code ec, std::size_t) {
                if (ec) return;
                self->do_write_download_chunks(hdr, sr, total, 0);
            });
    }

    void do_write_download_chunks(
        std::shared_ptr<http::response<http::buffer_body>> hdr,
        std::shared_ptr<http::response_serializer<http::buffer_body>> sr,
        std::uint64_t total,
        std::uint64_t written)
    {
        if (written >= total) {
            // Final empty chunk to signal end
            hdr->body().data = nullptr;
            hdr->body().size = 0;
            hdr->body().more = false;

            http::async_write(
                stream_, *sr,
                [self = shared_from_this(), hdr, sr](beast::error_code ec, std::size_t) {
                    if (ec) return;
                    self->do_read();
                });
            return;
        }

        constexpr std::size_t chunk_size = 8192;
        // Static buffer filled with 0x42
        static const auto fill_buf = []() {
            std::array<char, chunk_size> buf;
            buf.fill(0x42);
            return buf;
        }();

        auto remaining = total - written;
        auto to_write  = static_cast<std::size_t>(std::min<std::uint64_t>(remaining, chunk_size));
        bool is_last   = (written + to_write >= total);

        hdr->body().data = const_cast<char*>(fill_buf.data());
        hdr->body().size = to_write;
        hdr->body().more = !is_last;

        http::async_write(
            stream_, *sr,
            [self = shared_from_this(), hdr, sr, total, written, to_write](
                beast::error_code ec, std::size_t) {
                if (ec) return;
                auto new_written = written + to_write;
                if (new_written >= total) {
                    self->do_read();
                } else {
                    self->do_write_download_chunks(hdr, sr, total, new_written);
                }
            });
    }

    void do_close() {
        beast::get_lowest_layer(stream_).expires_after(std::chrono::seconds(5));
        stream_.async_shutdown(
            [self = shared_from_this()](beast::error_code) {});
    }
};

// ── Listener ────────────────────────────────────────────────────────────────

class listener : public std::enable_shared_from_this<listener> {
    net::io_context& ioc_;
    ssl::context& ctx_;
    tcp::acceptor acceptor_;

public:
    listener(net::io_context& ioc, ssl::context& ctx, tcp::endpoint endpoint)
        : ioc_(ioc), ctx_(ctx), acceptor_(net::make_strand(ioc)) {
        beast::error_code ec;
        acceptor_.open(endpoint.protocol(), ec);
        if (ec) { std::cerr << "open: " << ec.message() << "\n"; return; }

        acceptor_.set_option(net::socket_base::reuse_address(true), ec);
        acceptor_.bind(endpoint, ec);
        if (ec) { std::cerr << "bind: " << ec.message() << "\n"; return; }

        acceptor_.listen(net::socket_base::max_listen_connections, ec);
        if (ec) { std::cerr << "listen: " << ec.message() << "\n"; return; }
    }

    void run() { do_accept(); }

private:
    void do_accept() {
        acceptor_.async_accept(
            net::make_strand(ioc_),
            beast::bind_front_handler(&listener::on_accept, shared_from_this()));
    }

    void on_accept(beast::error_code ec, tcp::socket socket) {
        if (ec) {
            std::cerr << "accept: " << ec.message() << "\n";
        } else {
            std::make_shared<session>(std::move(socket), ctx_)->run();
        }
        do_accept();
    }
};

// ── Main ────────────────────────────────────────────────────────────────────

int main() {
    auto const cert_dir = []() -> std::string {
        if (auto* v = std::getenv("BENCH_CERT_DIR")) return v;
        return "/opt/bench";
    }();
    auto const cert_path = cert_dir + "/cert.pem";
    auto const key_path  = cert_dir + "/key.pem";
    auto const port = []() -> unsigned short {
        if (auto* v = std::getenv("BENCH_PORT")) return static_cast<unsigned short>(std::atoi(v));
        return 8443;
    }();

    auto const threads = std::max<int>(1, static_cast<int>(std::thread::hardware_concurrency()));

    // SSL context
    ssl::context ctx{ssl::context::tlsv12};
    ctx.set_options(
        ssl::context::default_workarounds |
        ssl::context::no_sslv2 |
        ssl::context::no_sslv3);
    ctx.use_certificate_chain_file(cert_path);
    ctx.use_private_key_file(key_path, ssl::context::pem);

    net::io_context ioc{threads};

    std::make_shared<listener>(
        ioc, ctx, tcp::endpoint{tcp::v4(), port})->run();

    std::cout << "C++ Boost.Beast server listening on port " << port
              << " (" << threads << " threads, C++" << __cplusplus << ")\n";

    // Run the I/O context on multiple threads
    std::vector<std::thread> v;
    v.reserve(threads - 1);
    for (auto i = threads - 1; i > 0; --i)
        v.emplace_back([&ioc] { ioc.run(); });
    ioc.run();

    return 0;
}
