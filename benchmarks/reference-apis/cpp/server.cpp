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
#include <fstream>
#include <iomanip>
#include <iostream>
#include <map>
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

// ── Leveled logging (all output to stderr) ─────────────────────────────────

enum LogLevel { LOG_ERROR = 0, LOG_WARN = 1, LOG_INFO = 2, LOG_DEBUG = 3 };
static LogLevel g_log_level = LOG_INFO;

static void bench_log(LogLevel level, const std::string& msg) {
    if (level > g_log_level) return;
    static const char* names[] = {"ERROR", "WARN", "INFO", "DEBUG"};
    std::cerr << "[" << names[level] << "] " << msg << std::endl;
}

static void init_log_level() {
    if (auto* env = std::getenv("LOG_LEVEL")) {
        std::string val(env);
        std::transform(val.begin(), val.end(), val.begin(), ::toupper);
        if (val == "ERROR") g_log_level = LOG_ERROR;
        else if (val == "WARN")  g_log_level = LOG_WARN;
        else if (val == "DEBUG") g_log_level = LOG_DEBUG;
        else g_log_level = LOG_INFO;
    }
}

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

// Bearer token authentication
static std::string bench_api_token;

// ── Shared benchmark dataset (loaded once at startup) ──────────────────────

struct BenchUser {
    int id;
    std::string name;
    std::string email;
    double score;
    std::string created_at;

    std::string to_json() const {
        std::ostringstream ss;
        ss << "{\"id\":" << id
           << ",\"name\":\"" << name << "\""
           << ",\"email\":\"" << email << "\""
           << ",\"score\":" << std::fixed << std::setprecision(2) << score
           << ",\"created_at\":\"" << created_at << "\"}";
        return ss.str();
    }
};

struct BenchTimeseriesPoint {
    double value;
    std::string category;
};

static bool bench_data_loaded = false;
static std::vector<BenchUser> bench_users;
static std::vector<std::string> bench_search_corpus;
static std::vector<BenchTimeseriesPoint> bench_timeseries;
static std::map<std::string, std::string> bench_checksums;

// Minimal JSON helpers for parsing bench-data.json
static std::string extract_json_string(const std::string& obj, const std::string& key) {
    std::string search = "\"" + key + "\"";
    auto pos = obj.find(search);
    if (pos == std::string::npos) return "";
    pos = obj.find(':', pos + search.size());
    if (pos == std::string::npos) return "";
    pos = obj.find('"', pos + 1);
    if (pos == std::string::npos) return "";
    auto end = pos + 1;
    while (end < obj.size() && !(obj[end] == '"' && obj[end - 1] != '\\')) ++end;
    return obj.substr(pos + 1, end - pos - 1);
}

static double extract_json_double(const std::string& obj, const std::string& key) {
    std::string search = "\"" + key + "\"";
    auto pos = obj.find(search);
    if (pos == std::string::npos) return 0.0;
    pos = obj.find(':', pos + search.size());
    if (pos == std::string::npos) return 0.0;
    ++pos;
    while (pos < obj.size() && (obj[pos] == ' ' || obj[pos] == '\t')) ++pos;
    auto end = pos;
    while (end < obj.size() && (std::isdigit(obj[end]) || obj[end] == '.' || obj[end] == '-' || obj[end] == 'e' || obj[end] == 'E' || obj[end] == '+')) ++end;
    try { return std::stod(obj.substr(pos, end - pos)); } catch (...) { return 0.0; }
}

static int extract_json_int(const std::string& obj, const std::string& key) {
    std::string search = "\"" + key + "\"";
    auto pos = obj.find(search);
    if (pos == std::string::npos) return 0;
    pos = obj.find(':', pos + search.size());
    if (pos == std::string::npos) return 0;
    ++pos;
    while (pos < obj.size() && (obj[pos] == ' ' || obj[pos] == '\t')) ++pos;
    auto end = pos;
    while (end < obj.size() && (std::isdigit(obj[end]) || obj[end] == '-')) ++end;
    try { return std::stoi(obj.substr(pos, end - pos)); } catch (...) { return 0; }
}

static size_t find_matching_bracket(const std::string& s, size_t open_pos, char open_ch, char close_ch) {
    if (open_pos >= s.size() || s[open_pos] != open_ch) return std::string::npos;
    int depth = 0;
    bool in_str = false;
    for (size_t i = open_pos; i < s.size(); ++i) {
        char c = s[i];
        if (c == '"' && (i == 0 || s[i - 1] != '\\')) in_str = !in_str;
        if (!in_str) {
            if (c == open_ch) ++depth;
            else if (c == close_ch) { --depth; if (depth == 0) return i; }
        }
    }
    return std::string::npos;
}

static void load_bench_data() {
    std::vector<std::string> paths;
    if (auto* env = std::getenv("BENCH_DATA_PATH")) {
        paths.emplace_back(env);
    }
    paths.emplace_back("/opt/bench/bench-data.json");
    paths.emplace_back("../shared/bench-data.json");

    for (const auto& p : paths) {
        std::ifstream file(p);
        if (!file.is_open()) continue;

        std::string content((std::istreambuf_iterator<char>(file)),
                            std::istreambuf_iterator<char>());
        file.close();

        // Parse users array
        auto users_key = content.find("\"users\"");
        if (users_key != std::string::npos) {
            auto arr_start = content.find('[', users_key);
            auto arr_end = find_matching_bracket(content, arr_start, '[', ']');
            if (arr_start != std::string::npos && arr_end != std::string::npos) {
                std::string arr = content.substr(arr_start + 1, arr_end - arr_start - 1);
                size_t pos = 0;
                while (pos < arr.size()) {
                    auto obj_start = arr.find('{', pos);
                    if (obj_start == std::string::npos) break;
                    auto obj_end = find_matching_bracket(arr, obj_start, '{', '}');
                    if (obj_end == std::string::npos) break;
                    std::string obj = arr.substr(obj_start + 1, obj_end - obj_start - 1);
                    BenchUser u;
                    u.id = extract_json_int(obj, "id");
                    u.name = extract_json_string(obj, "name");
                    u.email = extract_json_string(obj, "email");
                    u.score = extract_json_double(obj, "score");
                    u.created_at = extract_json_string(obj, "created_at");
                    bench_users.push_back(std::move(u));
                    pos = obj_end + 1;
                }
            }
        }

        // Parse search_corpus array (array of strings)
        auto search_key = content.find("\"search_corpus\"");
        if (search_key != std::string::npos) {
            auto arr_start = content.find('[', search_key);
            auto arr_end = find_matching_bracket(content, arr_start, '[', ']');
            if (arr_start != std::string::npos && arr_end != std::string::npos) {
                std::string arr = content.substr(arr_start + 1, arr_end - arr_start - 1);
                size_t pos = 0;
                while (pos < arr.size()) {
                    auto q_start = arr.find('"', pos);
                    if (q_start == std::string::npos) break;
                    auto q_end = q_start + 1;
                    while (q_end < arr.size() && !(arr[q_end] == '"' && arr[q_end - 1] != '\\')) ++q_end;
                    bench_search_corpus.push_back(arr.substr(q_start + 1, q_end - q_start - 1));
                    pos = q_end + 1;
                }
            }
        }

        // Parse timeseries array
        auto ts_key = content.find("\"timeseries\"");
        if (ts_key != std::string::npos) {
            auto arr_start = content.find('[', ts_key);
            auto arr_end = find_matching_bracket(content, arr_start, '[', ']');
            if (arr_start != std::string::npos && arr_end != std::string::npos) {
                std::string arr = content.substr(arr_start + 1, arr_end - arr_start - 1);
                size_t pos = 0;
                while (pos < arr.size()) {
                    auto obj_start = arr.find('{', pos);
                    if (obj_start == std::string::npos) break;
                    auto obj_end = find_matching_bracket(arr, obj_start, '{', '}');
                    if (obj_end == std::string::npos) break;
                    std::string obj = arr.substr(obj_start + 1, obj_end - obj_start - 1);
                    BenchTimeseriesPoint pt;
                    pt.value = extract_json_double(obj, "value");
                    pt.category = extract_json_string(obj, "category");
                    bench_timeseries.push_back(std::move(pt));
                    pos = obj_end + 1;
                }
            }
        }

        // Parse expected_checksums object
        auto cs_key = content.find("\"expected_checksums\"");
        if (cs_key != std::string::npos) {
            auto obj_start = content.find('{', cs_key);
            auto obj_end = find_matching_bracket(content, obj_start, '{', '}');
            if (obj_start != std::string::npos && obj_end != std::string::npos) {
                std::string obj = content.substr(obj_start + 1, obj_end - obj_start - 1);
                // Parse key-value pairs
                size_t pos = 0;
                while (pos < obj.size()) {
                    auto k_start = obj.find('"', pos);
                    if (k_start == std::string::npos) break;
                    auto k_end = obj.find('"', k_start + 1);
                    if (k_end == std::string::npos) break;
                    std::string key = obj.substr(k_start + 1, k_end - k_start - 1);
                    auto v_start = obj.find('"', k_end + 1);
                    // skip the colon, find next quote
                    v_start = obj.find('"', k_end + 2);
                    if (v_start == std::string::npos) break;
                    auto v_end = obj.find('"', v_start + 1);
                    if (v_end == std::string::npos) break;
                    bench_checksums[key] = obj.substr(v_start + 1, v_end - v_start - 1);
                    pos = v_end + 1;
                }
            }
        }

        bench_data_loaded = true;
        bench_log(LOG_INFO, "Loaded bench-data.json from " + p
                  + " (" + std::to_string(bench_users.size()) + " users, "
                  + std::to_string(bench_search_corpus.size()) + " corpus, "
                  + std::to_string(bench_timeseries.size()) + " timeseries)");
        return;
    }
    bench_log(LOG_WARN, "bench-data.json not found, falling back to per-language PRNG");
}

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
static void set_api_headers(auto& res, double duration_ms, double auth_dur_ms = -1) {
    res.set(http::field::content_type, "application/json");
    std::string timing = "app;dur=" + format_double(duration_ms, 1);
    if (auth_dur_ms >= 0) {
        timing = "auth;dur=" + format_double(auth_dur_ms, 1) + ", " + timing;
    }
    res.set("Server-Timing", timing);
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

    // Bearer token authentication
    double auth_dur_ms = -1;
    if (!bench_api_token.empty() && path != "/health") {
        double auth_t0 = now_ms();
        auto auth_it = req.find(http::field::authorization);
        std::string expected = "Bearer " + bench_api_token;
        if (auth_it == req.end() || std::string(auth_it->value()) != expected) {
            auth_dur_ms = now_ms() - auth_t0;
            http::response<http::string_body> res{http::status::unauthorized, req.version()};
            res.set(http::field::content_type, "application/json");
            res.set("Server-Timing", "auth;dur=" + format_double(auth_dur_ms, 1));
            res.keep_alive(req.keep_alive());
            res.body() = R"({"error":"unauthorized"})";
            res.prepare_payload();
            return send(std::move(res));
        }
        auth_dur_ms = now_ms() - auth_t0;
    }

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

        bool use_bench = bench_data_loaded && !bench_users.empty();
        std::string users_json;
        int total_users;

        if (use_bench) {
            // Use shared benchmark data — sort a copy of bench_users
            auto bu = bench_users; // copy for sorting
            if (sort_field == "name")
                std::sort(bu.begin(), bu.end(), [](auto& a, auto& b) { return a.name < b.name; });
            else if (sort_field == "email")
                std::sort(bu.begin(), bu.end(), [](auto& a, auto& b) { return a.email < b.email; });
            else if (sort_field == "score")
                std::sort(bu.begin(), bu.end(), [](auto& a, auto& b) { return a.score < b.score; });
            else
                std::sort(bu.begin(), bu.end(), [](auto& a, auto& b) { return a.id < b.id; });

            if (order == "desc")
                std::reverse(bu.begin(), bu.end());

            total_users = static_cast<int>(bu.size());
            int page_size = 20;
            int start_idx = (page - 1) * page_size;
            int end_idx = std::min(start_idx + page_size, total_users);

            users_json = "[";
            for (int i = start_idx; i < end_idx; ++i) {
                if (i > start_idx) users_json += ",";
                users_json += bu[i].to_json();
            }
            users_json += "]";
        } else {
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

            total_users = static_cast<int>(users.size());
            int page_size = 20;
            int start_idx = (page - 1) * page_size;
            int end_idx = std::min(start_idx + page_size, total_users);

            users_json = "[";
            for (int i = start_idx; i < end_idx; ++i) {
                if (i > start_idx) users_json += ",";
                users_json += users[i].to_json();
            }
            users_json += "]";
        }

        int page_size = 20;
        double duration_ms = now_ms() - t0;
        std::string body = "{\"page\":" + std::to_string(page) +
            ",\"page_size\":" + std::to_string(page_size) +
            ",\"total\":" + std::to_string(total_users) +
            ",\"sort\":\"" + sort_field + "\"" +
            ",\"order\":\"" + order + "\"" +
            ",\"users\":" + users_json + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms, auth_dur_ms);
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
            set_api_headers(res, duration_ms, auth_dur_ms);
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
        set_api_headers(res, duration_ms, auth_dur_ms);
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

        int count;
        std::vector<double> values;
        std::vector<int> assignments;

        if (bench_data_loaded && !bench_timeseries.empty()) {
            count = static_cast<int>(bench_timeseries.size());
            values.resize(count);
            assignments.resize(count);
            for (int i = 0; i < count; ++i) {
                values[i] = bench_timeseries[i].value;
                // Map category string to index
                int cat_idx = 0;
                for (int c = 0; c < NUM_CATS; ++c) {
                    if (bench_timeseries[i].category == CATEGORIES[c]) { cat_idx = c; break; }
                }
                assignments[i] = cat_idx;
            }
        } else {
            std::mt19937 rng(range_start);
            std::normal_distribution<double> dist(50.0, 15.0);
            count = 10000;
            values.resize(count);
            for (int i = 0; i < count; ++i)
                values[i] = dist(rng);

            std::mt19937 cat_rng(range_start + 1);
            assignments.resize(count);
            for (int i = 0; i < count; ++i)
                assignments[i] = cat_rng() % NUM_CATS;
        }

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
        set_api_headers(res, duration_ms, auth_dur_ms);
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

        struct CorpusItem { int id; std::string text; };
        std::vector<CorpusItem> corpus;

        if (bench_data_loaded && !bench_search_corpus.empty()) {
            corpus.reserve(bench_search_corpus.size());
            for (size_t i = 0; i < bench_search_corpus.size(); ++i) {
                corpus.push_back({static_cast<int>(i + 1), bench_search_corpus[i]});
            }
        } else {
            std::mt19937 rng(42);
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
        }

        // Case-insensitive literal search (std::regex is dangerously slow on pathological input)
        std::string query_lower = query;
        std::transform(query_lower.begin(), query_lower.end(), query_lower.begin(), ::tolower);

        struct Result { int id; std::string text; double score; };
        std::vector<Result> results;
        for (auto& item : corpus) {
            std::string text_lower = item.text;
            std::transform(text_lower.begin(), text_lower.end(), text_lower.begin(), ::tolower);
            auto pos = text_lower.find(query_lower);
            if (pos != std::string::npos) {
                double score = 1.0 / (1 + static_cast<double>(pos));
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
        set_api_headers(res, duration_ms, auth_dur_ms);
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
        set_api_headers(res, duration_ms, auth_dur_ms);
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
        set_api_headers(res, actual_ms, auth_dur_ms);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/validate?seed=42
    if (req.method() == http::verb::get && path == "/api/validate") {
        double t0 = now_ms();
        int seed = std::atoi(get_param(params, "seed", "42").c_str());

        std::string users_hash_str, agg_hash_str, search_hash_str;

        if (bench_data_loaded && !bench_checksums.empty()) {
            auto it_u = bench_checksums.find("users_page1");
            auto it_a = bench_checksums.find("aggregate_summary");
            auto it_s = bench_checksums.find("search_network_top10");
            users_hash_str = (it_u != bench_checksums.end()) ? it_u->second.substr(0, 16) : "";
            agg_hash_str = (it_a != bench_checksums.end()) ? it_a->second.substr(0, 16) : "";
            search_hash_str = (it_s != bench_checksums.end()) ? it_s->second.substr(0, 16) : "";
        } else {
            // Users checksum (page=1)
            std::mt19937 rng1(1);
            auto users = generate_users(rng1);
            std::string users_json = "[";
            for (size_t i = 0; i < users.size(); ++i) {
                if (i > 0) users_json += ",";
                users_json += users[i].to_sorted_json();
            }
            users_json += "]";
            users_hash_str = sha256_hex(users_json).substr(0, 16);

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
            agg_hash_str = sha256_hex(agg_json).substr(0, 16);

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
            search_hash_str = sha256_hex(search_json).substr(0, 16);
        }

        double duration_ms = now_ms() - t0;
        std::string body = "{\"seed\":" + std::to_string(seed) +
            ",\"checksums\":{\"users_page1\":\"" + users_hash_str + "\"" +
            ",\"aggregate_start0\":\"" + agg_hash_str + "\"" +
            ",\"search_corpus\":\"" + search_hash_str + "\"}}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, duration_ms, auth_dur_ms);
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
        if (ec) { bench_log(LOG_ERROR, "handshake: " + ec.message()); return; }
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
        if (ec) { bench_log(LOG_ERROR, "read: " + ec.message()); return; }

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
                        if (ec) { bench_log(LOG_ERROR, "write: " + ec.message()); return; }
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
                if (ec) { bench_log(LOG_ERROR, "write_header: " + ec.message()); return; }
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
                    if (ec) { bench_log(LOG_ERROR, "write: " + ec.message()); return; }
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
                if (ec) { bench_log(LOG_ERROR, "write: " + ec.message()); return; }
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
        if (ec) { bench_log(LOG_ERROR, "open: " + ec.message()); return; }

        acceptor_.set_option(net::socket_base::reuse_address(true), ec);
        acceptor_.bind(endpoint, ec);
        if (ec) { bench_log(LOG_ERROR, "bind: " + ec.message()); return; }

        acceptor_.listen(net::socket_base::max_listen_connections, ec);
        if (ec) { bench_log(LOG_ERROR, "listen: " + ec.message()); return; }
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
            bench_log(LOG_ERROR, "accept: " + ec.message());
        } else {
            std::make_shared<session>(std::move(socket), ctx_)->run();
        }
        do_accept();
    }
};

// ── Main ────────────────────────────────────────────────────────────────────

int main() {
    init_log_level();
    load_bench_data();

    // Bearer token auth
    const char* tok = getenv("BENCH_API_TOKEN");
    if (tok) bench_api_token = tok;

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

    bench_log(LOG_INFO, "C++ Boost.Beast server listening on port " + std::to_string(port)
              + " (" + std::to_string(threads) + " threads, C++" + std::to_string(__cplusplus) + ")");

    // Run the I/O context on multiple threads
    std::vector<std::thread> v;
    v.reserve(threads - 1);
    for (auto i = threads - 1; i > 0; --i)
        v.emplace_back([&ioc] { ioc.run(); });
    ioc.run();

    return 0;
}
