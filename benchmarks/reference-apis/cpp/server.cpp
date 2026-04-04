// server.cpp — C++ Boost.Beast + Boost.Asio SSL reference API for AletheBench.
//
// Endpoints:
//   GET  /health               → {"status":"ok","runtime":"cpp","version":"<__cplusplus>"}
//   GET  /download/{size}      → stream `size` bytes of 0x42 in 8192-byte chunks
//   POST /upload               → consume body, return {"bytes_received": N}
//   GET  /api/users            → paginated sorted user list
//   POST /api/transform        → SHA-256 hash strings, reverse values
//   GET  /api/aggregate        → stats over 10k generated points
//   GET  /api/search           → regex search over 1k generated strings
//   POST /api/upload/process   → CRC32 + SHA-256 + zlib compress body
//   GET  /api/delayed          → sleep with optional light work
//   GET  /api/validate         → checksums for verification
//
// Listens on port 8443 (or BENCH_PORT) with TLS.

#include <boost/asio.hpp>
#include <boost/asio/ssl.hpp>
#include <boost/beast.hpp>
#include <boost/beast/ssl.hpp>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iomanip>
#include <iostream>
#include <memory>
#include <numeric>
#include <random>
#include <regex>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include <openssl/sha.h>
#include <zlib.h>

namespace beast = boost::beast;
namespace http  = beast::http;
namespace net   = boost::asio;
namespace ssl   = net::ssl;
using tcp       = net::ip::tcp;

// ── Shared data ────────────────────────────────────────────────────────────

static const char* FIRST_NAMES[] = {
    "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Heidi",
    "Ivan", "Judy", "Karl", "Laura", "Mallory", "Nina", "Oscar", "Peggy",
    "Quentin", "Ruth", "Steve", "Trent", "Ursula", "Victor", "Wendy",
    "Xander", "Yvonne", "Zack",
};
static const int NUM_FIRST = 26;

static const char* LAST_NAMES[] = {
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
    "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
    "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
};
static const int NUM_LAST = 20;

static const char* DEPARTMENTS[] = {
    "Engineering", "Marketing", "Sales", "Finance", "HR",
    "Operations", "Legal", "Support", "Design", "Product",
};
static const int NUM_DEPT = 10;

static const char* SEARCH_WORDS[] = {
    "network", "latency", "throughput", "bandwidth", "packet",
    "routing", "firewall", "proxy", "endpoint", "server",
    "client", "protocol", "socket", "buffer", "stream",
    "timeout", "retry", "cache", "queue", "load",
};
static const int NUM_WORDS = 20;

static const char* CATEGORIES[] = {"alpha", "beta", "gamma", "delta", "epsilon"};
static const int NUM_CATS = 5;

// ── Helpers ────────────────────────────────────────────────────────────────

static std::string cplusplus_version() {
    return std::to_string(__cplusplus);
}

static std::string json_health() {
    return R"({"status":"ok","runtime":"cpp","version":")" + cplusplus_version() + R"("})";
}

static std::string json_bytes_received(std::uint64_t n) {
    return R"({"bytes_received":)" + std::to_string(n) + "}";
}

static std::string sha256_hex(const std::string& data) {
    unsigned char hash[SHA256_DIGEST_LENGTH];
    SHA256(reinterpret_cast<const unsigned char*>(data.data()), data.size(), hash);
    std::ostringstream ss;
    ss << std::hex << std::setfill('0');
    for (int i = 0; i < SHA256_DIGEST_LENGTH; ++i)
        ss << std::setw(2) << static_cast<int>(hash[i]);
    return ss.str();
}

static std::string sha256_hex(const char* data, size_t len) {
    unsigned char hash[SHA256_DIGEST_LENGTH];
    SHA256(reinterpret_cast<const unsigned char*>(data), len, hash);
    std::ostringstream ss;
    ss << std::hex << std::setfill('0');
    for (int i = 0; i < SHA256_DIGEST_LENGTH; ++i)
        ss << std::setw(2) << static_cast<int>(hash[i]);
    return ss.str();
}

static double now_ms() {
    return std::chrono::duration<double, std::milli>(
        std::chrono::steady_clock::now().time_since_epoch()
    ).count();
}

static std::string format_double(double v, int precision) {
    std::ostringstream ss;
    ss << std::fixed << std::setprecision(precision) << v;
    return ss.str();
}

// JSON string escaping
static std::string json_escape(const std::string& s) {
    std::string out;
    out.reserve(s.size() + 8);
    for (char c : s) {
        switch (c) {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n";  break;
            case '\r': out += "\\r";  break;
            case '\t': out += "\\t";  break;
            default:   out += c;      break;
        }
    }
    return out;
}

// Parse query string into key-value pairs
static std::vector<std::pair<std::string, std::string>> parse_query(beast::string_view target) {
    std::vector<std::pair<std::string, std::string>> params;
    auto qpos = target.find('?');
    if (qpos == beast::string_view::npos) return params;

    std::string qs(target.substr(qpos + 1));
    std::istringstream stream(qs);
    std::string pair;
    while (std::getline(stream, pair, '&')) {
        auto eq = pair.find('=');
        if (eq != std::string::npos)
            params.emplace_back(pair.substr(0, eq), pair.substr(eq + 1));
        else
            params.emplace_back(pair, "");
    }
    return params;
}

static std::string get_param(const std::vector<std::pair<std::string, std::string>>& params,
                             const std::string& key, const std::string& def = "") {
    for (auto& [k, v] : params)
        if (k == key) return v;
    return def;
}

// Get path portion (before '?')
static std::string get_path(beast::string_view target) {
    auto qpos = target.find('?');
    if (qpos == beast::string_view::npos) return std::string(target);
    return std::string(target.substr(0, qpos));
}

// Parse a path like "/download/1048576" and extract the size.
static std::uint64_t parse_download_size(beast::string_view target) {
    constexpr beast::string_view prefix = "/download/";
    if (target.size() <= prefix.size())
        return 0;
    if (target.substr(0, prefix.size()) != prefix)
        return 0;
    auto num_str = target.substr(prefix.size());
    auto qpos = num_str.find('?');
    if (qpos != beast::string_view::npos)
        num_str = num_str.substr(0, qpos);
    try {
        return std::stoull(std::string(num_str));
    } catch (...) {
        return 0;
    }
}

// Set API response headers
static void set_api_headers(auto& res, double duration_ms) {
    res.set(http::field::content_type, "application/json");
    res.set("Server-Timing", "app;dur=" + format_double(duration_ms, 1));
    res.set(http::field::cache_control, "no-store, no-cache, must-revalidate");
    res.set("Timing-Allow-Origin", "*");
    res.set(http::field::access_control_allow_origin, "*");
}

// ── User generation ────────────────────────────────────────────────────────

struct User {
    int id;
    std::string name;
    std::string email;
    int age;
    std::string department;
    double score;

    std::string to_json() const {
        return "{\"id\":" + std::to_string(id) +
               ",\"name\":\"" + json_escape(name) + "\"" +
               ",\"email\":\"" + json_escape(email) + "\"" +
               ",\"age\":" + std::to_string(age) +
               ",\"department\":\"" + json_escape(department) + "\"" +
               ",\"score\":" + format_double(score, 2) + "}";
    }

    // For sorted JSON (key-sorted) used in validate checksum
    std::string to_sorted_json() const {
        return "{\"age\":" + std::to_string(age) +
               ",\"department\":\"" + json_escape(department) + "\"" +
               ",\"email\":\"" + json_escape(email) + "\"" +
               ",\"id\":" + std::to_string(id) +
               ",\"name\":\"" + json_escape(name) + "\"" +
               ",\"score\":" + format_double(score, 2) + "}";
    }
};

static std::vector<User> generate_users(std::mt19937& rng, int count = 100) {
    std::vector<User> users;
    users.reserve(count);
    for (int i = 0; i < count; ++i) {
        User u;
        u.id = i + 1;
        u.name = std::string(FIRST_NAMES[rng() % NUM_FIRST]) + " " + LAST_NAMES[rng() % NUM_LAST];
        u.email = "user" + std::to_string(i + 1) + "@example.com";
        u.age = 22 + static_cast<int>(rng() % 44); // 22..65
        u.department = DEPARTMENTS[rng() % NUM_DEPT];
        u.score = std::round((static_cast<double>(rng() % 10001) / 100.0) * 100.0) / 100.0;
        users.push_back(std::move(u));
    }
    return users;
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
    auto path = get_path(target);
    auto params = parse_query(target);

    // GET /health
    if (req.method() == http::verb::get && path == "/health") {
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

        http::response<http::buffer_body> res;
        res.result(http::status::ok);
        res.version(req.version());
        res.set(http::field::content_type, "application/octet-stream");
        res.content_length(size);
        res.keep_alive(req.keep_alive());

        res.body().data = nullptr;
        res.body().size = 0;
        res.body().more = true;

        return send(std::move(res), size);
    }

    // POST /upload
    if (req.method() == http::verb::post && path == "/upload") {
        auto bytes = req.body().size();
        http::response<http::string_body> res{http::status::ok, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = json_bytes_received(bytes);
        res.prepare_payload();
        return send(std::move(res));
    }

    // ── JSON API endpoints ─────────────────────────────────────────────────

    // GET /api/users?page=N&sort=field&order=asc|desc
    if (req.method() == http::verb::get && path == "/api/users") {
        double t0 = now_ms();
        int page = std::max(1, std::atoi(get_param(params, "page", "1").c_str()));
        std::string sort_field = get_param(params, "sort", "id");
        std::string order = get_param(params, "order", "asc");

        std::mt19937 rng(page);
        auto users = generate_users(rng);

        // Sort
        if (sort_field == "id")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.id < b.id; });
        else if (sort_field == "name")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.name < b.name; });
        else if (sort_field == "email")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.email < b.email; });
        else if (sort_field == "age")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.age < b.age; });
        else if (sort_field == "department")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.department < b.department; });
        else if (sort_field == "score")
            std::sort(users.begin(), users.end(), [](auto& a, auto& b) { return a.score < b.score; });

        if (order == "desc")
            std::reverse(users.begin(), users.end());

        int page_size = 20;
        int start = (page - 1) * page_size;
        int end_idx = std::min(start + page_size, static_cast<int>(users.size()));

        std::string users_json = "[";
        for (int i = start; i < end_idx; ++i) {
            if (i > start) users_json += ",";
            users_json += users[i].to_json();
        }
        users_json += "]";

        double duration_ms = now_ms() - t0;
        std::string body = "{\"page\":" + std::to_string(page) +
            ",\"page_size\":" + std::to_string(page_size) +
            ",\"total\":" + std::to_string(users.size()) +
            ",\"sort\":\"" + sort_field + "\"" +
            ",\"order\":\"" + order + "\"" +
            ",\"users\":" + users_json + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // POST /api/transform
    if (req.method() == http::verb::post && path == "/api/transform") {
        double t0 = now_ms();
        auto& raw = req.body();

        // Minimal JSON object parser: expects {"key":"value", ...}
        // We parse key-value pairs where values can be strings or other types
        if (raw.empty() || raw[0] != '{') {
            double duration_ms = now_ms() - t0;
            http::response<http::string_body> res{http::status::bad_request, req.version()};
            set_api_headers(res, duration_ms);
            res.keep_alive(req.keep_alive());
            res.body() = R"({"error":"invalid JSON"})";
            res.prepare_payload();
            return send(std::move(res));
        }

        // Simple JSON key-value extraction
        struct KV { std::string key; std::string value; bool is_string; };
        std::vector<KV> kvs;
        size_t pos = 1; // skip '{'
        while (pos < raw.size()) {
            // skip whitespace/commas
            while (pos < raw.size() && (raw[pos] == ' ' || raw[pos] == ',' || raw[pos] == '\n' || raw[pos] == '\r' || raw[pos] == '\t'))
                ++pos;
            if (pos >= raw.size() || raw[pos] == '}') break;

            // expect key as string
            if (raw[pos] != '"') break;
            ++pos;
            std::string key;
            while (pos < raw.size() && raw[pos] != '"') {
                if (raw[pos] == '\\' && pos + 1 < raw.size()) { key += raw[pos + 1]; pos += 2; }
                else { key += raw[pos]; ++pos; }
            }
            if (pos < raw.size()) ++pos; // skip closing "

            // skip : and whitespace
            while (pos < raw.size() && (raw[pos] == ':' || raw[pos] == ' ')) ++pos;

            // parse value
            if (pos < raw.size() && raw[pos] == '"') {
                // string value
                ++pos;
                std::string val;
                while (pos < raw.size() && raw[pos] != '"') {
                    if (raw[pos] == '\\' && pos + 1 < raw.size()) { val += raw[pos + 1]; pos += 2; }
                    else { val += raw[pos]; ++pos; }
                }
                if (pos < raw.size()) ++pos; // skip closing "
                kvs.push_back({key, val, true});
            } else {
                // non-string value (number, bool, null, array, object)
                std::string val;
                int depth = 0;
                while (pos < raw.size()) {
                    char c = raw[pos];
                    if (c == '{' || c == '[') ++depth;
                    else if (c == '}' || c == ']') {
                        if (depth == 0) break;
                        --depth;
                    } else if ((c == ',' || c == ' ') && depth == 0) break;
                    val += c;
                    ++pos;
                }
                kvs.push_back({key, val, false});
            }
        }

        std::string transformed = "{";
        for (size_t i = 0; i < kvs.size(); ++i) {
            if (i > 0) transformed += ",";
            transformed += "\"" + json_escape(kvs[i].key) + "\":";
            if (kvs[i].is_string) {
                std::string reversed(kvs[i].value.rbegin(), kvs[i].value.rend());
                std::string hashed = sha256_hex(kvs[i].value);
                transformed += "{\"original_reversed\":\"" + json_escape(reversed) +
                              "\",\"sha256\":\"" + hashed + "\"}";
            } else {
                transformed += kvs[i].value;
            }
        }
        transformed += "}";

        double duration_ms = now_ms() - t0;
        std::string body = "{\"original_fields\":" + std::to_string(kvs.size()) +
            ",\"transformed\":" + transformed + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/aggregate?range=start,end
    if (req.method() == http::verb::get && path == "/api/aggregate") {
        double t0 = now_ms();
        std::string range_param = get_param(params, "range", "0,1000");
        int range_start = 0, range_end = 1000;
        auto comma = range_param.find(',');
        if (comma != std::string::npos) {
            range_start = std::atoi(range_param.substr(0, comma).c_str());
            range_end = std::atoi(range_param.substr(comma + 1).c_str());
        } else {
            range_start = std::atoi(range_param.c_str());
        }

        std::mt19937 rng(range_start);
        std::normal_distribution<double> dist(50.0, 15.0);
        int count = 10000;
        std::vector<double> values(count);
        for (int i = 0; i < count; ++i)
            values[i] = dist(rng);

        std::mt19937 cat_rng(range_start + 1);
        std::vector<int> assignments(count);
        for (int i = 0; i < count; ++i)
            assignments[i] = cat_rng() % NUM_CATS;

        auto sorted_vals = values;
        std::sort(sorted_vals.begin(), sorted_vals.end());
        double mean = std::accumulate(values.begin(), values.end(), 0.0) / count;
        double p50 = sorted_vals[count / 2];
        double p95 = sorted_vals[static_cast<int>(count * 0.95)];
        double max_val = sorted_vals.back();

        std::string groups = "{";
        bool first_group = true;
        for (int c = 0; c < NUM_CATS; ++c) {
            std::vector<double> cat_vals;
            for (int i = 0; i < count; ++i)
                if (assignments[i] == c) cat_vals.push_back(values[i]);
            if (cat_vals.empty()) continue;

            std::sort(cat_vals.begin(), cat_vals.end());
            double cat_mean = std::accumulate(cat_vals.begin(), cat_vals.end(), 0.0) / cat_vals.size();

            if (!first_group) groups += ",";
            first_group = false;
            groups += "\"" + std::string(CATEGORIES[c]) + "\":{" +
                "\"count\":" + std::to_string(cat_vals.size()) +
                ",\"mean\":" + format_double(std::round(cat_mean * 10000) / 10000, 4) +
                ",\"p50\":" + format_double(std::round(cat_vals[cat_vals.size() / 2] * 10000) / 10000, 4) +
                ",\"max\":" + format_double(std::round(cat_vals.back() * 10000) / 10000, 4) + "}";
        }
        groups += "}";

        double duration_ms = now_ms() - t0;
        std::string body = "{\"range\":{\"start\":" + std::to_string(range_start) +
            ",\"end\":" + std::to_string(range_end) + "}" +
            ",\"total_points\":" + std::to_string(count) +
            ",\"stats\":{\"mean\":" + format_double(std::round(mean * 10000) / 10000, 4) +
            ",\"p50\":" + format_double(std::round(p50 * 10000) / 10000, 4) +
            ",\"p95\":" + format_double(std::round(p95 * 10000) / 10000, 4) +
            ",\"max\":" + format_double(std::round(max_val * 10000) / 10000, 4) + "}" +
            ",\"groups\":" + groups + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/search?q=term&limit=N
    if (req.method() == http::verb::get && path == "/api/search") {
        double t0 = now_ms();
        std::string query = get_param(params, "q", "test");
        int limit = std::max(1, std::atoi(get_param(params, "limit", "10").c_str()));

        std::mt19937 rng(42);
        struct CorpusItem { int id; std::string text; };
        std::vector<CorpusItem> corpus;
        corpus.reserve(1000);
        for (int i = 0; i < 1000; ++i) {
            int word_count = 3 + static_cast<int>(rng() % 6); // 3..8
            std::string phrase;
            for (int j = 0; j < word_count; ++j) {
                if (j > 0) phrase += " ";
                phrase += SEARCH_WORDS[rng() % NUM_WORDS];
            }
            corpus.push_back({i + 1, std::move(phrase)});
        }

        std::regex pattern;
        try {
            pattern = std::regex(query, std::regex_constants::icase);
        } catch (const std::regex_error&) {
            double duration_ms = now_ms() - t0;
            http::response<http::string_body> res{http::status::bad_request, req.version()};
            set_api_headers(res, duration_ms);
            res.keep_alive(req.keep_alive());
            res.body() = R"({"error":"invalid regex"})";
            res.prepare_payload();
            return send(std::move(res));
        }

        struct Result { int id; std::string text; double score; };
        std::vector<Result> results;
        for (auto& item : corpus) {
            std::smatch match;
            if (std::regex_search(item.text, match, pattern)) {
                double score = 1.0 / (1 + match.position(0));
                results.push_back({item.id, item.text, std::round(score * 10000) / 10000});
            }
        }

        std::sort(results.begin(), results.end(), [](auto& a, auto& b) { return a.score > b.score; });
        if (static_cast<int>(results.size()) > limit)
            results.resize(limit);

        std::string results_json = "[";
        for (size_t i = 0; i < results.size(); ++i) {
            if (i > 0) results_json += ",";
            results_json += "{\"id\":" + std::to_string(results[i].id) +
                ",\"text\":\"" + json_escape(results[i].text) + "\"" +
                ",\"score\":" + format_double(results[i].score, 4) + "}";
        }
        results_json += "]";

        double duration_ms = now_ms() - t0;
        std::string body = "{\"query\":\"" + json_escape(query) + "\"" +
            ",\"total_matches\":" + std::to_string(results.size()) +
            ",\"limit\":" + std::to_string(limit) +
            ",\"results\":" + results_json + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // POST /api/upload/process
    if (req.method() == http::verb::post && path == "/api/upload/process") {
        double t0 = now_ms();
        auto& body_data = req.body();

        // CRC32
        uLong crc = crc32(0L, Z_NULL, 0);
        crc = crc32(crc, reinterpret_cast<const Bytef*>(body_data.data()), static_cast<uInt>(body_data.size()));

        // SHA-256
        std::string sha = sha256_hex(body_data.data(), body_data.size());

        // zlib compress
        uLongf comp_len = compressBound(static_cast<uLong>(body_data.size()));
        std::vector<Bytef> compressed(comp_len);
        compress(compressed.data(), &comp_len, reinterpret_cast<const Bytef*>(body_data.data()),
                 static_cast<uLong>(body_data.size()));

        size_t orig_size = body_data.size();
        size_t comp_size = static_cast<size_t>(comp_len);
        double ratio = static_cast<double>(comp_size) / std::max(orig_size, static_cast<size_t>(1));

        // Format CRC32 as hex
        char crc_hex[9];
        std::snprintf(crc_hex, sizeof(crc_hex), "%08lx", static_cast<unsigned long>(crc & 0xFFFFFFFF));

        double duration_ms = now_ms() - t0;
        std::string body = "{\"original_size\":" + std::to_string(orig_size) +
            ",\"compressed_size\":" + std::to_string(comp_size) +
            ",\"compression_ratio\":" + format_double(std::round(ratio * 10000) / 10000, 4) +
            ",\"crc32\":\"" + std::string(crc_hex) + "\"" +
            ",\"sha256\":\"" + sha + "\"}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/delayed?ms=N&work=light
    if (req.method() == http::verb::get && path == "/api/delayed") {
        double t0 = now_ms();
        int ms = std::max(1, std::min(100, std::atoi(get_param(params, "ms", "100").c_str())));
        std::string work = get_param(params, "work", "none");

        std::this_thread::sleep_for(std::chrono::milliseconds(ms));

        if (work == "light") {
            std::string data;
            for (int i = 0; i < 100; ++i) data += "benchmark";
            sha256_hex(data);
        }

        double actual_ms = now_ms() - t0;
        std::string body = "{\"requested_ms\":" + std::to_string(ms) +
            ",\"actual_ms\":" + format_double(std::round(actual_ms * 100) / 100, 2) +
            ",\"work\":\"" + work + "\"}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, actual_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/validate?seed=42
    if (req.method() == http::verb::get && path == "/api/validate") {
        double t0 = now_ms();
        int seed = std::atoi(get_param(params, "seed", "42").c_str());

        // Users checksum (page=1)
        std::mt19937 rng1(1);
        auto users = generate_users(rng1);
        std::string users_json = "[";
        for (size_t i = 0; i < users.size(); ++i) {
            if (i > 0) users_json += ",";
            users_json += users[i].to_sorted_json();
        }
        users_json += "]";
        std::string users_hash = sha256_hex(users_json);

        // Aggregate checksum (start=0)
        std::mt19937 rng2(0);
        std::normal_distribution<double> dist(50.0, 15.0);
        std::vector<double> values(10000);
        for (int i = 0; i < 10000; ++i)
            values[i] = std::round(dist(rng2) * 10000) / 10000;
        std::sort(values.begin(), values.end());
        std::string agg_json = "[";
        for (size_t i = 0; i < values.size(); ++i) {
            if (i > 0) agg_json += ",";
            agg_json += format_double(values[i], 4);
        }
        agg_json += "]";
        std::string agg_hash = sha256_hex(agg_json);

        // Search checksum
        std::mt19937 rng3(42);
        std::string search_json = "[";
        for (int i = 0; i < 1000; ++i) {
            int word_count = 3 + static_cast<int>(rng3() % 6);
            std::string phrase;
            for (int j = 0; j < word_count; ++j) {
                if (j > 0) phrase += " ";
                phrase += SEARCH_WORDS[rng3() % NUM_WORDS];
            }
            if (i > 0) search_json += ",";
            search_json += "\"" + json_escape(phrase) + "\"";
        }
        search_json += "]";
        std::string search_hash = sha256_hex(search_json);

        double duration_ms = now_ms() - t0;
        std::string body = "{\"seed\":" + std::to_string(seed) +
            ",\"checksums\":{\"users_page1\":\"" + users_hash.substr(0, 16) + "\"" +
            ",\"aggregate_start0\":\"" + agg_hash.substr(0, 16) + "\"" +
            ",\"search_corpus\":\"" + search_hash.substr(0, 16) + "\"}}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
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
        if (ec) { std::cerr << "handshake: " << ec.message() << "\n"; return; }
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
        if (ec) { std::cerr << "read: " << ec.message() << "\n"; return; }

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
                        if (ec) { std::cerr << "write: " << ec.message() << "\n"; return; }
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
                if (ec) { std::cerr << "write_header: " << ec.message() << "\n"; return; }
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
                    if (ec) { std::cerr << "write: " << ec.message() << "\n"; return; }
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
                if (ec) { std::cerr << "write: " << ec.message() << "\n"; return; }
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
    ssl::context ctx{ssl::context::tls_server};
    ctx.set_options(
        ssl::context::default_workarounds |
        ssl::context::no_sslv2 |
        ssl::context::no_sslv3 |
        ssl::context::no_tlsv1 |
        ssl::context::no_tlsv1_1);
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
