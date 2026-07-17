// server.cpp — C++ Boost.Beast + Boost.Asio SSL reference API for AletheBench.
//
// Conforms to benchmarks/shared/API-SPEC.md (frozen contract v1, family C).
//
// Endpoints:
//   GET  /health               → byte-constant {"status":"ok","runtime":"cpp","version":...}
//   GET  /download/{size}      → exactly `size` bytes of 0x42 in 8192-byte chunks
//   POST /upload               → incremental drain, {"received_bytes":N}
//   GET  /api/users            → bare array: sorted 100-user window, first 20
//   POST /api/transform        → {"seed","hashed_fields","reversed_values"}
//   GET  /api/aggregate        → quintile stats over the 10k shared timeseries
//   GET  /api/search           → positional regex search over the shared corpus
//   POST /api/upload/process   → CRC32 + SHA-256 + zlib(RFC 1950, level 6)
//   GET  /api/delayed          → asio steady_timer delay, ms clamped [1,100]
//   GET  /api/validate         → {"seed","checksums":<dataset expected_checksums>}
//
// Worker policy (spec §3): io_context pool threads = BENCH_WORKERS
// (default = logical CPU count). Listens on BENCH_PORT (default 8443) TLS.
// Dataset load failure is FATAL (spec §2) — there is no PRNG fallback.

#include <boost/asio.hpp>
#include <boost/asio/ssl.hpp>
#include <boost/beast.hpp>
#include <boost/beast/ssl.hpp>
#include <algorithm>
#include <array>
#include <cctype>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <map>
#include <memory>
#include <optional>
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

[[noreturn]] static void bench_fatal(const std::string& msg) {
    std::cerr << "FATAL: " << msg << std::endl;
    std::exit(1);
}

// ── Constants (spec §5.2) ───────────────────────────────────────────────────

static constexpr std::size_t   DOWNLOAD_CHUNK = 8192;        // pinned chunk size
static constexpr std::uint64_t DOWNLOAD_CAP   = 2147483648ULL; // 2 GiB cap
static constexpr char          DOWNLOAD_FILL  = 0x42;        // pinned fill byte

// Bearer token authentication
static std::string bench_api_token;

// /health body — byte-constant, precomputed at startup (spec §5.1).
static std::string g_health_body;

// ── Shared benchmark dataset (spec §2, loaded once, failure = fatal) ───────

struct BenchUser {
    int id;
    std::string name;
    std::string email;
    double score;
    std::string created_at;
};

static std::vector<BenchUser> bench_users;
static std::vector<std::string> bench_search_corpus;
static std::vector<double> bench_ts_values; // dataset order (spec §5.6)
static std::map<std::string, std::string> bench_checksums;

// Minimal JSON helpers for the fixed bench-data.json schema.
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
    while (end < obj.size() &&
           (std::isdigit(static_cast<unsigned char>(obj[end])) || obj[end] == '.' ||
            obj[end] == '-' || obj[end] == 'e' || obj[end] == 'E' || obj[end] == '+'))
        ++end;
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
    while (end < obj.size() &&
           (std::isdigit(static_cast<unsigned char>(obj[end])) || obj[end] == '-'))
        ++end;
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

// Count top-level objects inside the array at `key` (schema verification).
static size_t count_array_objects(const std::string& content, const std::string& key) {
    auto k = content.find("\"" + key + "\"");
    if (k == std::string::npos) return 0;
    auto arr_start = content.find('[', k);
    auto arr_end = find_matching_bracket(content, arr_start, '[', ']');
    if (arr_start == std::string::npos || arr_end == std::string::npos) return 0;
    size_t count = 0, pos = arr_start + 1;
    while (pos < arr_end) {
        auto obj_start = content.find('{', pos);
        if (obj_start == std::string::npos || obj_start > arr_end) break;
        auto obj_end = find_matching_bracket(content, obj_start, '{', '}');
        if (obj_end == std::string::npos) break;
        ++count;
        pos = obj_end + 1;
    }
    return count;
}

static void parse_dataset(const std::string& content, const std::string& source) {
    // users
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

    // search_corpus (array of strings)
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

    // timeseries — only the `value` field is used (spec §5.6), dataset order.
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
                bench_ts_values.push_back(extract_json_double(obj, "value"));
                pos = obj_end + 1;
            }
        }
    }

    // expected_checksums
    auto cs_key = content.find("\"expected_checksums\"");
    if (cs_key != std::string::npos) {
        auto obj_start = content.find('{', cs_key);
        auto obj_end = find_matching_bracket(content, obj_start, '{', '}');
        if (obj_start != std::string::npos && obj_end != std::string::npos) {
            std::string obj = content.substr(obj_start + 1, obj_end - obj_start - 1);
            size_t pos = 0;
            while (pos < obj.size()) {
                auto k_start = obj.find('"', pos);
                if (k_start == std::string::npos) break;
                auto k_end = obj.find('"', k_start + 1);
                if (k_end == std::string::npos) break;
                std::string key = obj.substr(k_start + 1, k_end - k_start - 1);
                auto colon = obj.find(':', k_end + 1);
                if (colon == std::string::npos) break;
                auto v_start = obj.find('"', colon + 1);
                if (v_start == std::string::npos) break;
                auto v_end = obj.find('"', v_start + 1);
                if (v_end == std::string::npos) break;
                bench_checksums[key] = obj.substr(v_start + 1, v_end - v_start - 1);
                pos = v_end + 1;
            }
        }
    }

    // Verify the §2 schema counts — exit non-zero on mismatch.
    if (extract_json_int(content, "_version") != 2)
        bench_fatal("bench-data.json at " + source + ": _version != 2");
    if (bench_users.size() != 100)
        bench_fatal("bench-data.json at " + source + ": users != 100 (got " +
                    std::to_string(bench_users.size()) + ")");
    if (bench_search_corpus.size() != 1000)
        bench_fatal("bench-data.json at " + source + ": search_corpus != 1000 (got " +
                    std::to_string(bench_search_corpus.size()) + ")");
    if (bench_ts_values.size() != 10000)
        bench_fatal("bench-data.json at " + source + ": timeseries != 10000 (got " +
                    std::to_string(bench_ts_values.size()) + ")");
    if (count_array_objects(content, "transform_inputs") != 10)
        bench_fatal("bench-data.json at " + source + ": transform_inputs != 10");
    if (bench_checksums.size() != 4)
        bench_fatal("bench-data.json at " + source + ": expected_checksums != 4 keys");
}

// Spec §2 resolution order; failure is fatal, no PRNG fallback.
static void load_bench_data() {
    std::string path;
    if (auto* env = std::getenv("BENCH_DATA_PATH"); env && *env) {
        // An explicitly configured path must exist and parse.
        std::ifstream file(env);
        if (!file.is_open())
            bench_fatal(std::string("BENCH_DATA_PATH=") + env + " could not be opened");
        path = env;
    } else {
        for (const char* p : {"/opt/bench/bench-data.json", "../shared/bench-data.json"}) {
            std::ifstream file(p);
            if (file.is_open()) { path = p; break; }
        }
        if (path.empty())
            bench_fatal("bench-data.json not found (set BENCH_DATA_PATH or deploy "
                        "/opt/bench/bench-data.json); reference implementations have "
                        "no PRNG fallback (spec §2)");
    }

    std::ifstream file(path);
    std::string content((std::istreambuf_iterator<char>(file)),
                        std::istreambuf_iterator<char>());
    file.close();
    if (content.empty())
        bench_fatal("bench-data.json at " + path + " is empty or unreadable");

    parse_dataset(content, path);
    bench_log(LOG_INFO, "Loaded bench-data.json from " + path +
              " (_version 2, 100 users, 1000 corpus, 10000 timeseries)");
}

// ── Helpers ────────────────────────────────────────────────────────────────

static std::string sha256_hex(const char* data, size_t len) {
    unsigned char hash[SHA256_DIGEST_LENGTH];
    SHA256(reinterpret_cast<const unsigned char*>(data), len, hash);
    std::ostringstream ss;
    ss << std::hex << std::setfill('0');
    for (int i = 0; i < SHA256_DIGEST_LENGTH; ++i)
        ss << std::setw(2) << static_cast<int>(hash[i]);
    return ss.str();
}

static std::string sha256_hex(const std::string& data) {
    return sha256_hex(data.data(), data.size());
}

static double now_ms() {
    return std::chrono::duration<double, std::milli>(
        std::chrono::steady_clock::now().time_since_epoch()
    ).count();
}

// Spec §5.6: round half away from zero to 2 decimals (float64 semantics).
static double r2(double x) {
    return std::floor(x * 100.0 + 0.5) / 100.0;
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
            default:
                if (static_cast<unsigned char>(c) < 0x20) {
                    char buf[8];
                    std::snprintf(buf, sizeof(buf), "\\u%04x", c);
                    out += buf;
                } else {
                    out += c;
                }
                break;
        }
    }
    return out;
}

static std::string url_decode(const std::string& s) {
    std::string out;
    out.reserve(s.size());
    for (size_t i = 0; i < s.size(); ++i) {
        char c = s[i];
        if (c == '+') {
            out += ' ';
        } else if (c == '%' && i + 2 < s.size() &&
                   std::isxdigit(static_cast<unsigned char>(s[i + 1])) &&
                   std::isxdigit(static_cast<unsigned char>(s[i + 2]))) {
            out += static_cast<char>(std::stoi(s.substr(i + 1, 2), nullptr, 16));
            i += 2;
        } else {
            out += c;
        }
    }
    return out;
}

// Parse query string into decoded key-value pairs
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
            params.emplace_back(url_decode(pair.substr(0, eq)), url_decode(pair.substr(eq + 1)));
        else
            params.emplace_back(url_decode(pair), "");
    }
    return params;
}

static std::string get_param(const std::vector<std::pair<std::string, std::string>>& params,
                             const std::string& key, const std::string& def = "") {
    for (auto& [k, v] : params)
        if (k == key) return v;
    return def;
}

static int int_param(const std::vector<std::pair<std::string, std::string>>& params,
                     const std::string& key, int def) {
    for (auto& [k, v] : params) {
        if (k != key) continue;
        int out{};
        auto [p, ec] = std::from_chars(v.data(), v.data() + v.size(), out);
        if (ec == std::errc() && p == v.data() + v.size()) return out;
        return def;
    }
    return def;
}

// Get path portion (before '?')
static std::string get_path(beast::string_view target) {
    auto qpos = target.find('?');
    if (qpos == beast::string_view::npos) return std::string(target);
    return std::string(target.substr(0, qpos));
}

// Parse "/download/{size}": nullopt = non-integer (→400), value clamped to cap.
static std::optional<std::uint64_t> parse_download_size(const std::string& path) {
    constexpr std::string_view prefix = "/download/";
    std::string num = path.substr(prefix.size());
    if (num.empty()) return std::nullopt;
    std::uint64_t v{};
    auto [p, ec] = std::from_chars(num.data(), num.data() + num.size(), v);
    if (ec != std::errc() || p != num.data() + num.size()) return std::nullopt;
    return std::min<std::uint64_t>(v, DOWNLOAD_CAP);
}

// The four benchmark headers required on every /api/* response (spec §1).
static void set_api_headers(auto& res, double duration_ms) {
    res.set(http::field::content_type, "application/json");
    res.set("Server-Timing", "app;dur=" + format_double(duration_ms, 1));
    res.set(http::field::cache_control, "no-store, no-cache, must-revalidate");
    res.set("Timing-Allow-Origin", "*");
    res.set(http::field::access_control_allow_origin, "*");
}

static bool authorized(const http::fields& f) {
    if (bench_api_token.empty()) return true;
    auto it = f.find(http::field::authorization);
    if (it == f.end()) return false;
    return std::string(it->value()) == "Bearer " + bench_api_token;
}

// ── Minimal top-level JSON object scanner (for /api/transform) ─────────────
//
// Captures the raw value text of each top-level key. Returns false when the
// body is not a syntactically plausible JSON object (→ 400).

static bool parse_top_level_object(const std::string& raw,
                                   std::vector<std::pair<std::string, std::string>>& out) {
    size_t pos = 0;
    auto skip_ws = [&] {
        while (pos < raw.size() && (raw[pos] == ' ' || raw[pos] == '\t' ||
                                    raw[pos] == '\n' || raw[pos] == '\r'))
            ++pos;
    };
    skip_ws();
    if (pos >= raw.size() || raw[pos] != '{') return false;
    ++pos;
    skip_ws();
    if (pos < raw.size() && raw[pos] == '}') return true; // empty object

    while (pos < raw.size()) {
        skip_ws();
        if (pos >= raw.size() || raw[pos] != '"') return false;
        ++pos;
        std::string key;
        while (pos < raw.size() && raw[pos] != '"') {
            if (raw[pos] == '\\' && pos + 1 < raw.size()) { key += raw[pos + 1]; pos += 2; }
            else { key += raw[pos]; ++pos; }
        }
        if (pos >= raw.size()) return false;
        ++pos; // closing "
        skip_ws();
        if (pos >= raw.size() || raw[pos] != ':') return false;
        ++pos;
        skip_ws();
        if (pos >= raw.size()) return false;

        std::string value;
        if (raw[pos] == '"') {
            auto start = pos;
            ++pos;
            while (pos < raw.size() && !(raw[pos] == '"' && raw[pos - 1] != '\\')) ++pos;
            if (pos >= raw.size()) return false;
            ++pos;
            value = raw.substr(start, pos - start);
        } else if (raw[pos] == '{' || raw[pos] == '[') {
            char open = raw[pos];
            char close = (open == '{') ? '}' : ']';
            auto end = find_matching_bracket(raw, pos, open, close);
            if (end == std::string::npos) return false;
            value = raw.substr(pos, end - pos + 1);
            pos = end + 1;
        } else {
            auto start = pos;
            while (pos < raw.size() && raw[pos] != ',' && raw[pos] != '}' &&
                   raw[pos] != ' ' && raw[pos] != '\t' && raw[pos] != '\n' && raw[pos] != '\r')
                ++pos;
            value = raw.substr(start, pos - start);
            if (value.empty()) return false;
        }
        out.emplace_back(std::move(key), std::move(value));
        skip_ws();
        if (pos < raw.size() && raw[pos] == ',') { ++pos; continue; }
        if (pos < raw.size() && raw[pos] == '}') return true;
        return false;
    }
    return false;
}

// Split a raw JSON array body "[a,b,...]" into raw element tokens.
static bool split_json_array(const std::string& raw, std::vector<std::string>& out) {
    size_t pos = 0;
    while (pos < raw.size() && std::isspace(static_cast<unsigned char>(raw[pos]))) ++pos;
    if (pos >= raw.size() || raw[pos] != '[') return false;
    auto arr_end = find_matching_bracket(raw, pos, '[', ']');
    if (arr_end == std::string::npos) return false;
    ++pos;
    while (pos < arr_end) {
        while (pos < arr_end && (std::isspace(static_cast<unsigned char>(raw[pos])) || raw[pos] == ',')) ++pos;
        if (pos >= arr_end) break;
        size_t start = pos;
        if (raw[pos] == '"') {
            ++pos;
            while (pos < arr_end && !(raw[pos] == '"' && raw[pos - 1] != '\\')) ++pos;
            ++pos;
        } else if (raw[pos] == '{' || raw[pos] == '[') {
            char open = raw[pos];
            char close = (open == '{') ? '}' : ']';
            auto end = find_matching_bracket(raw, pos, open, close);
            if (end == std::string::npos || end > arr_end) return false;
            pos = end + 1;
        } else {
            while (pos < arr_end && raw[pos] != ',' &&
                   !std::isspace(static_cast<unsigned char>(raw[pos])))
                ++pos;
        }
        std::string tok = raw.substr(start, pos - start);
        if (!tok.empty()) out.push_back(std::move(tok));
    }
    return true;
}

// Unescape a raw JSON string token (with surrounding quotes) to its text.
static std::string unquote_json_string(const std::string& tok) {
    if (tok.size() < 2 || tok.front() != '"') return tok;
    std::string out;
    std::size_t end = tok.size() - 1; // index of the closing quote
    for (std::size_t i = 1; i < end; ++i) {
        if (tok[i] == '\\' && i + 1 < end) {
            char n = tok[i + 1];
            switch (n) {
                case 'n': out += '\n'; break;
                case 'r': out += '\r'; break;
                case 't': out += '\t'; break;
                default:  out += n;    break;
            }
            ++i;
        } else {
            out += tok[i];
        }
    }
    return out;
}

// ── Request handler (everything except /upload and /api/delayed, which the
//    session handles itself for streaming drain / timer-based sleep) ────────

template <class Send>
void handle_request(http::request<http::string_body>&& req, Send&& send) {
    auto const json_error = [&req](http::status status, beast::string_view why) {
        http::response<http::string_body> res{status, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = R"({"error":")" + std::string(why) + R"("})";
        res.prepare_payload();
        return res;
    };

    auto target = req.target();
    auto path = get_path(target);
    auto params = parse_query(target);

    // GET /health — auth-exempt, constant-work (spec §5.1).
    if (req.method() == http::verb::get && path == "/health") {
        http::response<http::string_body> res{http::status::ok, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = g_health_body;
        res.prepare_payload();
        return send(std::move(res));
    }

    // Bearer token authentication — every other route (spec §1).
    if (!authorized(req)) {
        http::response<http::string_body> res{http::status::unauthorized, req.version()};
        res.set(http::field::content_type, "application/json");
        res.keep_alive(req.keep_alive());
        res.body() = R"({"error":"unauthorized"})";
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /download/{size} (spec §5.2).
    if (req.method() == http::verb::get && path.rfind("/download/", 0) == 0) {
        double t0 = now_ms();
        auto size = parse_download_size(path);
        if (!size) { // non-integer → 400
            return send(json_error(http::status::bad_request, "invalid size"));
        }

        http::response<http::buffer_body> res;
        res.result(http::status::ok);
        res.version(req.version());
        res.set(http::field::content_type, "application/octet-stream");
        res.set("X-Download-Bytes", std::to_string(*size));
        res.set("Server-Timing", "proc;dur=" + format_double(now_ms() - t0, 1));
        res.content_length(*size);
        res.keep_alive(req.keep_alive());

        res.body().data = nullptr;
        res.body().size = 0;
        res.body().more = true;

        return send(std::move(res), *size);
    }

    // ── JSON API endpoints (family C, spec §5.4-§5.10) ─────────────────────

    // GET /api/users?page=N&sort=<field>&order=<asc|desc>
    if (req.method() == http::verb::get && path == "/api/users") {
        double t0 = now_ms();
        std::int64_t page = std::max(1, int_param(params, "page", 1));
        std::string sort_field = get_param(params, "sort", "id");
        bool desc = get_param(params, "order", "asc") == "desc";

        // 100-user window of the dataset; page beyond it → [] with 200.
        std::vector<BenchUser> window;
        std::size_t start = static_cast<std::size_t>(page - 1) * 100;
        if (start < bench_users.size()) {
            std::size_t end = std::min(start + 100, bench_users.size());
            window.assign(bench_users.begin() + start, bench_users.begin() + end);
        }

        enum Field { F_ID, F_NAME, F_EMAIL, F_SCORE, F_CREATED };
        Field f = F_ID;
        if (sort_field == "name") f = F_NAME;
        else if (sort_field == "email") f = F_EMAIL;
        else if (sort_field == "score") f = F_SCORE;
        else if (sort_field == "created_at") f = F_CREATED;
        // (unrecognized values fall back to id)

        // Stable sort — dataset order breaks ties; desc reverses the
        // comparator so ties stay in dataset order (family C semantics).
        std::stable_sort(window.begin(), window.end(),
            [f, desc](const BenchUser& a, const BenchUser& b) {
                int c = 0;
                switch (f) {
                    case F_NAME:    c = a.name.compare(b.name); break;       // bytewise
                    case F_EMAIL:   c = a.email.compare(b.email); break;
                    case F_CREATED: c = a.created_at.compare(b.created_at); break;
                    case F_SCORE:   c = (a.score < b.score) ? -1 : (a.score > b.score ? 1 : 0); break;
                    case F_ID:      c = (a.id < b.id) ? -1 : (a.id > b.id ? 1 : 0); break;
                }
                return desc ? c > 0 : c < 0;
            });

        // Bare JSON array of the first 20 users of the sorted window.
        std::string body = "[";
        std::size_t take = std::min<std::size_t>(20, window.size());
        for (std::size_t i = 0; i < take; ++i) {
            if (i > 0) body += ",";
            const auto& u = window[i];
            body += "{\"id\":" + std::to_string(u.id) +
                    ",\"name\":\"" + json_escape(u.name) + "\"" +
                    ",\"email\":\"" + json_escape(u.email) + "\"" +
                    ",\"score\":" + format_double(u.score, 2) +
                    ",\"created_at\":\"" + json_escape(u.created_at) + "\"}";
        }
        body += "]";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // POST /api/transform — {"seed","hashed_fields","reversed_values"}.
    if (req.method() == http::verb::post && path == "/api/transform") {
        double t0 = now_ms();

        std::vector<std::pair<std::string, std::string>> kvs;
        if (!parse_top_level_object(req.body(), kvs)) { // invalid JSON → 400
            http::response<http::string_body> res{http::status::bad_request, req.version()};
            set_api_headers(res, now_ms() - t0);
            res.keep_alive(req.keep_alive());
            res.body() = R"({"error":"invalid JSON"})";
            res.prepare_payload();
            return send(std::move(res));
        }

        std::string seed = "0";
        std::vector<std::string> fields;
        std::vector<std::string> values;
        for (auto& [k, v] : kvs) {
            if (k == "seed" && !v.empty() && v != "null") {
                seed = v;
            } else if (k == "fields") {
                std::vector<std::string> toks;
                if (split_json_array(v, toks))
                    for (auto& t : toks) fields.push_back(unquote_json_string(t));
            } else if (k == "values") {
                split_json_array(v, values); // raw tokens, passed through unmodified
            }
        }

        std::string hashed = "[";
        for (std::size_t i = 0; i < fields.size(); ++i) {
            if (i > 0) hashed += ",";
            hashed += "\"" + sha256_hex(fields[i]) + "\"";
        }
        hashed += "]";

        std::string reversed = "[";
        for (std::size_t i = 0; i < values.size(); ++i) {
            if (i > 0) reversed += ",";
            reversed += values[values.size() - 1 - i];
        }
        reversed += "]";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = "{\"seed\":" + seed +
                     ",\"hashed_fields\":" + hashed +
                     ",\"reversed_values\":" + reversed + "}";
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/aggregate — `range` accepted and ignored (spec §5.6).
    if (req.method() == http::verb::get && path == "/api/aggregate") {
        double t0 = now_ms();

        auto values = bench_ts_values; // copy: sorting is part of the workload
        std::sort(values.begin(), values.end());
        std::size_t n = values.size();
        double total = 0.0;
        for (double v : values) total += v; // sequential sum over SORTED values

        std::size_t chunk = n / 5;
        std::string categories = "[";
        for (std::size_t i = 0; i < 5; ++i) {
            double s = 0.0;
            for (std::size_t j = i * chunk; j < (i + 1) * chunk; ++j) s += values[j];
            if (i > 0) categories += ",";
            categories += "{\"category\":\"q" + std::to_string(i + 1) + "\"" +
                          ",\"count\":" + std::to_string(chunk) +
                          ",\"mean\":" + format_double(r2(s / static_cast<double>(chunk)), 2) +
                          ",\"min\":" + format_double(r2(values[i * chunk]), 2) +
                          ",\"max\":" + format_double(r2(values[(i + 1) * chunk - 1]), 2) + "}";
        }
        categories += "]";

        std::string body = "{\"total_points\":" + std::to_string(n) +
            ",\"mean\":" + format_double(r2(total / static_cast<double>(n)), 2) +
            ",\"p50\":" + format_double(r2(values[static_cast<std::size_t>(static_cast<double>(n) * 0.50)]), 2) +
            ",\"p95\":" + format_double(r2(values[static_cast<std::size_t>(static_cast<double>(n) * 0.95)]), 2) +
            ",\"max\":" + format_double(r2(values[n - 1]), 2) +
            ",\"categories\":" + categories + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/search?q=<term>&limit=N (spec §5.7).
    if (req.method() == http::verb::get && path == "/api/search") {
        double t0 = now_ms();
        std::string query = get_param(params, "q", "test");
        int limit = std::min(int_param(params, "limit", 20), 100);
        if (limit < 0) limit = 0;

        // Case-sensitive regex; literal substring fallback on invalid pattern.
        std::optional<std::regex> re;
        try {
            re.emplace(query);
        } catch (const std::regex_error&) {
            re.reset();
        }

        std::vector<std::pair<std::size_t, const std::string*>> matches;
        for (const auto& item : bench_search_corpus) {
            std::size_t pos;
            if (re) {
                std::smatch m;
                if (!std::regex_search(item, m, *re)) continue;
                pos = static_cast<std::size_t>(m.position(0));
            } else {
                auto found = item.find(query);
                if (found == std::string::npos) continue;
                pos = found;
            }
            matches.emplace_back(pos, &item);
        }
        std::sort(matches.begin(), matches.end(),
            [](const auto& a, const auto& b) {
                if (a.first != b.first) return a.first < b.first; // position asc
                return *a.second < *b.second;                     // item asc bytewise
            });

        std::size_t take = std::min<std::size_t>(static_cast<std::size_t>(limit), matches.size());
        std::string results = "[";
        for (std::size_t i = 0; i < take; ++i) {
            if (i > 0) results += ",";
            results += "{\"rank\":" + std::to_string(i + 1) +
                       ",\"item\":\"" + json_escape(*matches[i].second) + "\"" +
                       ",\"match_position\":" + std::to_string(matches[i].first) + "}";
        }
        results += "]";

        std::string body = "{\"query\":\"" + json_escape(query) + "\"" +
            ",\"total_matches\":" + std::to_string(matches.size()) + // BEFORE truncation
            ",\"returned\":" + std::to_string(take) +
            ",\"results\":" + results + "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // POST /api/upload/process (spec §5.8).
    if (req.method() == http::verb::post && path == "/api/upload/process") {
        double t0 = now_ms();
        auto& body_data = req.body();

        uLong crc = crc32(0L, Z_NULL, 0);
        crc = crc32(crc, reinterpret_cast<const Bytef*>(body_data.data()),
                    static_cast<uInt>(body_data.size()));

        std::string sha = sha256_hex(body_data.data(), body_data.size());

        // zlib (RFC 1950) at level 6.
        uLongf comp_len = compressBound(static_cast<uLong>(body_data.size()));
        std::vector<Bytef> compressed(comp_len);
        compress2(compressed.data(), &comp_len,
                  reinterpret_cast<const Bytef*>(body_data.data()),
                  static_cast<uLong>(body_data.size()), 6);

        char crc_hex[9];
        std::snprintf(crc_hex, sizeof(crc_hex), "%08lx",
                      static_cast<unsigned long>(crc & 0xFFFFFFFF));

        std::string body = "{\"original_size\":" + std::to_string(body_data.size()) +
            ",\"compressed_size\":" + std::to_string(static_cast<std::size_t>(comp_len)) +
            ",\"crc32\":\"" + std::string(crc_hex) + "\"" +
            ",\"sha256\":\"" + sha + "\"}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = std::move(body);
        res.prepare_payload();
        return send(std::move(res));
    }

    // GET /api/validate?seed=N — echo dataset checksums (spec §5.10).
    if (req.method() == http::verb::get && path == "/api/validate") {
        double t0 = now_ms();
        int seed = int_param(params, "seed", 42);

        std::string checksums = "{";
        bool first = true;
        for (const auto& [k, v] : bench_checksums) {
            if (!first) checksums += ",";
            first = false;
            checksums += "\"" + json_escape(k) + "\":\"" + json_escape(v) + "\"";
        }
        checksums += "}";

        http::response<http::string_body> res{http::status::ok, req.version()};
        set_api_headers(res, now_ms() - t0);
        res.keep_alive(req.keep_alive());
        res.body() = "{\"seed\":" + std::to_string(seed) +
                     ",\"checksums\":" + checksums + "}";
        res.prepare_payload();
        return send(std::move(res));
    }

    return send(json_error(http::status::not_found, "not found"));
}

// ── Session ─────────────────────────────────────────────────────────────────

class session : public std::enable_shared_from_this<session> {
    beast::ssl_stream<beast::tcp_stream> stream_;
    beast::flat_buffer buffer_;
    // Header-first parsing: /upload drains its body incrementally through
    // drain_buf_ (spec §5.3 — no wholesale buffering); everything else is
    // converted to a string-body parser.
    std::optional<http::request_parser<http::buffer_body>> hdr_parser_;
    std::optional<http::request_parser<http::string_body>> body_parser_;
    std::array<char, 65536> drain_buf_{};
    std::uint64_t upload_received_ = 0;
    double upload_t0_ = 0;
    bool keep_alive_ = false;
    unsigned version_ = 11;
    bool has_request_id_ = false;
    std::string request_id_;

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
        hdr_parser_.emplace();
        body_parser_.reset();
        hdr_parser_->body_limit(DOWNLOAD_CAP); // spec cap; must be set pre-parse
        beast::get_lowest_layer(stream_).expires_after(std::chrono::seconds(30));
        http::async_read_header(
            stream_, buffer_, *hdr_parser_,
            beast::bind_front_handler(&session::on_read_header, shared_from_this()));
    }

    void on_read_header(beast::error_code ec, std::size_t /*bytes*/) {
        if (ec == http::error::end_of_stream)
            return do_close();
        if (ec) { bench_log(LOG_ERROR, "read_header: " + ec.message()); return; }

        auto const& hreq = hdr_parser_->get();
        std::string path = get_path(hreq.target());

        // POST /upload — drain incrementally, never buffering the whole body.
        if (hreq.method() == http::verb::post && path == "/upload") {
            if (!authorized(hreq)) {
                http::response<http::string_body> res{http::status::unauthorized, hreq.version()};
                res.set(http::field::content_type, "application/json");
                res.keep_alive(false); // body not drained → connection must close
                res.body() = R"({"error":"unauthorized"})";
                res.prepare_payload();
                return write_response(std::move(res));
            }
            upload_received_ = 0;
            upload_t0_ = now_ms();
            keep_alive_ = hreq.keep_alive();
            version_ = hreq.version();
            auto it = hreq.find("X-Networker-Request-Id");
            has_request_id_ = (it != hreq.end());
            if (has_request_id_) request_id_ = std::string(it->value());
            return drain_upload();
        }

        // Everything else: read the remaining body into a string.
        body_parser_.emplace(std::move(*hdr_parser_));
        hdr_parser_.reset();
        http::async_read(
            stream_, buffer_, *body_parser_,
            beast::bind_front_handler(&session::on_read_full, shared_from_this()));
    }

    void drain_upload() {
        if (hdr_parser_->is_done())
            return respond_upload();

        hdr_parser_->get().body().data = drain_buf_.data();
        hdr_parser_->get().body().size = drain_buf_.size();
        beast::get_lowest_layer(stream_).expires_after(std::chrono::seconds(30));
        http::async_read(
            stream_, buffer_, *hdr_parser_,
            [self = shared_from_this()](beast::error_code ec, std::size_t) {
                if (ec == http::error::need_buffer) ec = {};
                if (ec) { bench_log(LOG_ERROR, "upload read: " + ec.message()); return; }
                self->upload_received_ +=
                    self->drain_buf_.size() - self->hdr_parser_->get().body().size;
                self->drain_upload();
            });
    }

    void respond_upload() {
        double recv_ms = now_ms() - upload_t0_;
        http::response<http::string_body> res{http::status::ok, version_};
        res.set(http::field::content_type, "application/json");
        res.set("X-Networker-Received-Bytes", std::to_string(upload_received_));
        res.set("Server-Timing", "recv;dur=" + format_double(recv_ms, 1));
        if (has_request_id_)
            res.set("X-Networker-Request-Id", request_id_);
        res.keep_alive(keep_alive_);
        res.body() = "{\"received_bytes\":" + std::to_string(upload_received_) + "}";
        res.prepare_payload();
        write_response(std::move(res));
    }

    void on_read_full(beast::error_code ec, std::size_t /*bytes*/) {
        if (ec == http::error::end_of_stream)
            return do_close();
        if (ec) { bench_log(LOG_ERROR, "read: " + ec.message()); return; }

        auto req = body_parser_->release();
        body_parser_.reset();

        // HEAD is served by the GET handler with the body stripped (the
        // validator's header checks use HEAD, mirroring axum auto-HEAD).
        bool const is_head = req.method() == http::verb::head;
        if (is_head)
            req.method(http::verb::get);

        // GET /api/delayed — asio timer, never blocks a pool thread (spec §5.9).
        if (req.method() == http::verb::get && get_path(req.target()) == "/api/delayed")
            return handle_delayed(std::move(req));

        handle_request(std::move(req),
            [this, is_head](auto&& msg, std::uint64_t download_size = 0) {
                using msg_type = std::decay_t<decltype(msg)>;
                if (is_head)
                    return write_head_response(msg);
                if constexpr (std::is_same_v<msg_type, http::response<http::buffer_body>>) {
                    do_write_download(std::move(msg), download_size);
                } else {
                    write_response(std::move(msg));
                }
            });
    }

    // Send only the headers of `res` (HEAD semantics: same headers, no body).
    template <class Res>
    void write_head_response(Res& res) {
        http::response<http::empty_body> h;
        h.result(res.result());
        h.version(res.version());
        for (auto const& f : res.base())
            h.set(f.name_string(), f.value());
        h.keep_alive(res.keep_alive());
        write_response(std::move(h));
    }

    void handle_delayed(http::request<http::string_body>&& req) {
        double t0 = now_ms();
        if (!authorized(req)) {
            http::response<http::string_body> res{http::status::unauthorized, req.version()};
            res.set(http::field::content_type, "application/json");
            res.keep_alive(req.keep_alive());
            res.body() = R"({"error":"unauthorized"})";
            res.prepare_payload();
            return write_response(std::move(res));
        }

        auto params = parse_query(req.target());
        int ms = std::clamp(int_param(params, "ms", 10), 1, 100); // spec §5.9 clamp
        // `work` is reserved: accepted and ignored.
        bool keep = req.keep_alive();
        unsigned ver = req.version();

        auto timer = std::make_shared<net::steady_timer>(stream_.get_executor());
        timer->expires_after(std::chrono::milliseconds(ms));
        timer->async_wait(
            [self = shared_from_this(), timer, ms, t0, keep, ver](beast::error_code ec) {
                if (ec) { bench_log(LOG_ERROR, "delayed timer: " + ec.message()); return; }
                double actual = now_ms() - t0;
                http::response<http::string_body> res{http::status::ok, ver};
                set_api_headers(res, actual);
                res.keep_alive(keep);
                res.body() = "{\"requested_ms\":" + std::to_string(ms) +
                             ",\"actual_ms\":" + format_double(actual, 2) + "}";
                res.prepare_payload();
                self->write_response(std::move(res));
            });
    }

    template <class Response>
    void write_response(Response&& msg) {
        auto sp = std::make_shared<std::decay_t<Response>>(std::forward<Response>(msg));
        http::async_write(
            stream_, *sp,
            [self = shared_from_this(), sp](beast::error_code ec, std::size_t) {
                if (ec) { bench_log(LOG_ERROR, "write: " + ec.message()); return; }
                if (sp->need_eof())
                    return self->do_close();
                self->do_read();
            });
    }

    void do_write_download(http::response<http::buffer_body> header, std::uint64_t total) {
        // Multi-GiB downloads legitimately exceed the 30 s read deadline;
        // the next do_read() re-arms it.
        beast::get_lowest_layer(stream_).expires_never();
        auto hdr = std::make_shared<http::response<http::buffer_body>>(std::move(header));
        auto sr  = std::make_shared<http::response_serializer<http::buffer_body>>(*hdr);

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

        // Static buffer filled with 0x42 (spec §5.2: pinned fill + chunk size).
        static const auto fill_buf = []() {
            std::array<char, DOWNLOAD_CHUNK> buf{};
            buf.fill(DOWNLOAD_FILL);
            return buf;
        }();

        auto remaining = total - written;
        auto to_write  = static_cast<std::size_t>(std::min<std::uint64_t>(remaining, DOWNLOAD_CHUNK));
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
    load_bench_data(); // fatal on failure (spec §2)

    // /health byte-constant body (spec §5.1).
    g_health_body = std::string(R"({"status":"ok","runtime":"cpp","version":")") +
                    std::to_string(__cplusplus) + "\"}";

    // Bearer token auth
    if (const char* tok = std::getenv("BENCH_API_TOKEN"))
        bench_api_token = tok;

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

    // Worker policy (spec §3): io_context pool threads = BENCH_WORKERS,
    // default = logical CPU count.
    auto const threads = []() -> int {
        if (auto* v = std::getenv("BENCH_WORKERS")) {
            int n = std::atoi(v);
            if (n > 0) return n;
        }
        return std::max<int>(1, static_cast<int>(std::thread::hardware_concurrency()));
    }();

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

    // Run the I/O context on the worker thread pool
    std::vector<std::thread> v;
    v.reserve(threads - 1);
    for (auto i = threads - 1; i > 0; --i)
        v.emplace_back([&ioc] { ioc.run(); });
    ioc.run();

    return 0;
}
