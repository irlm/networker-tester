import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpHandler;
import com.sun.net.httpserver.HttpsConfigurator;
import com.sun.net.httpserver.HttpsParameters;
import com.sun.net.httpserver.HttpsServer;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyFactory;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.spec.PKCS8EncodedKeySpec;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.logging.ConsoleHandler;
import java.util.logging.Level;
import java.util.logging.Logger;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.regex.PatternSyntaxException;
import java.util.zip.CRC32;
import java.util.zip.Deflater;

/**
 * AletheBench Java reference API.
 *
 * Single-file HTTPS server using JDK built-in com.sun.net.httpserver.
 * No external dependencies. Conforms to the frozen contract in
 * benchmarks/shared/API-SPEC.md (family C).
 *
 * Worker policy (API-SPEC.md §3): the HttpServer executor is a fixed thread
 * pool sized by BENCH_WORKERS (default = logical CPU count). /api/delayed is
 * completed from a scheduler thread so the sleep never blocks a pool worker.
 *
 * Endpoints:
 *   GET  /health                -> byte-constant {"status","runtime","version"}
 *   GET  /download/{size}       -> `size` bytes of 0x42 in 8 KiB chunks
 *   POST /upload                -> drain body, {"received_bytes": N}
 *   GET  /api/users             -> bare array, first 20 of sorted 100-user window
 *   POST /api/transform         -> {seed, hashed_fields, reversed_values}
 *   GET  /api/aggregate         -> quintile stats over the full timeseries
 *   GET  /api/search            -> {query, total_matches, returned, results}
 *   POST /api/upload/process    -> {original_size, compressed_size, crc32, sha256}
 *   GET  /api/delayed           -> async timer delay, clamped [1, 100] ms
 *   GET  /api/validate          -> {seed, checksums} from the shared dataset
 */
public class Server {

    private static final int PORT = Integer.parseInt(System.getenv().getOrDefault("BENCH_PORT", "8443"));
    private static final String CERT_DIR = System.getenv().getOrDefault("BENCH_CERT_DIR", "/opt/bench");
    private static final String BENCH_TOKEN = System.getenv("BENCH_API_TOKEN");
    private static final long MAX_DOWNLOAD = 2_147_483_648L; // §5.2: 2 GiB clamp
    private static final int CHUNK_SIZE = 8192;              // §5.2: 8 KiB chunks

    private static final Logger logger = Logger.getLogger("bench-api");

    static class JsonFormatter extends java.util.logging.Formatter {
        @Override
        public String format(java.util.logging.LogRecord record) {
            String level = record.getLevel().getName().toLowerCase();
            if ("severe".equals(level)) level = "error";
            if ("warning".equals(level)) level = "warn";
            return String.format(
                "{\"ts\":\"%s\",\"service\":\"java\",\"level\":\"%s\",\"message\":\"%s\"}\n",
                java.time.Instant.ofEpochMilli(record.getMillis()).toString(),
                level,
                record.getMessage().replace("\"", "\\\"")
            );
        }
    }

    static {
        // Direct all logging to stderr with configurable level via LOG_LEVEL env var.
        Logger rootLogger = Logger.getLogger("");
        for (var h : rootLogger.getHandlers()) rootLogger.removeHandler(h);
        ConsoleHandler handler = new ConsoleHandler(); // writes to stderr by default
        handler.setLevel(Level.ALL);

        String logFormat = System.getenv("LOG_FORMAT");
        if ("json".equals(logFormat)) {
            handler.setFormatter(new JsonFormatter());
        }

        rootLogger.addHandler(handler);

        String envLevel = System.getenv("LOG_LEVEL");
        if (envLevel != null) {
            try {
                Level level = Level.parse(envLevel.toUpperCase());
                logger.setLevel(level);
                handler.setLevel(level);
            } catch (IllegalArgumentException ignored) {
                // keep default INFO
            }
        }
    }

    // ── Minimal but CORRECT JSON parser + writer ──────────────────────
    //
    // The previous hand-written scanner broke on escaped quotes (audit F5).
    // This is a complete recursive-descent JSON implementation: objects →
    // LinkedHashMap, arrays → ArrayList, strings with full escape/\-uXXXX
    // handling, numbers → Long when integral in the source text, Double
    // otherwise (so 57.61 round-trips as 57.61 and 39.0 keeps its ".0",
    // matching the §7 canonical-JSON float semantics).

    static final class Json {
        static Object parse(String s) {
            P p = new P(s);
            p.ws();
            Object v = p.value();
            p.ws();
            if (p.pos != s.length()) throw new IllegalArgumentException("trailing data at " + p.pos);
            return v;
        }

        private static final class P {
            final String s;
            int pos = 0;
            P(String s) { this.s = s; }

            void ws() {
                while (pos < s.length()) {
                    char c = s.charAt(pos);
                    if (c == ' ' || c == '\t' || c == '\n' || c == '\r') pos++;
                    else break;
                }
            }

            char peek() {
                if (pos >= s.length()) throw new IllegalArgumentException("unexpected end of input");
                return s.charAt(pos);
            }

            void expect(char c) {
                if (peek() != c) throw new IllegalArgumentException("expected '" + c + "' at " + pos);
                pos++;
            }

            Object value() {
                char c = peek();
                switch (c) {
                    case '{': return object();
                    case '[': return array();
                    case '"': return string();
                    case 't': literal("true"); return Boolean.TRUE;
                    case 'f': literal("false"); return Boolean.FALSE;
                    case 'n': literal("null"); return null;
                    default: return number();
                }
            }

            void literal(String lit) {
                if (!s.startsWith(lit, pos)) throw new IllegalArgumentException("invalid literal at " + pos);
                pos += lit.length();
            }

            Map<String, Object> object() {
                expect('{');
                Map<String, Object> m = new LinkedHashMap<>();
                ws();
                if (peek() == '}') { pos++; return m; }
                while (true) {
                    ws();
                    String key = string();
                    ws();
                    expect(':');
                    ws();
                    m.put(key, value());
                    ws();
                    char c = peek();
                    if (c == ',') { pos++; continue; }
                    if (c == '}') { pos++; return m; }
                    throw new IllegalArgumentException("expected ',' or '}' at " + pos);
                }
            }

            List<Object> array() {
                expect('[');
                List<Object> a = new ArrayList<>();
                ws();
                if (peek() == ']') { pos++; return a; }
                while (true) {
                    ws();
                    a.add(value());
                    ws();
                    char c = peek();
                    if (c == ',') { pos++; continue; }
                    if (c == ']') { pos++; return a; }
                    throw new IllegalArgumentException("expected ',' or ']' at " + pos);
                }
            }

            String string() {
                expect('"');
                StringBuilder sb = new StringBuilder();
                while (true) {
                    if (pos >= s.length()) throw new IllegalArgumentException("unterminated string");
                    char c = s.charAt(pos++);
                    if (c == '"') return sb.toString();
                    if (c == '\\') {
                        if (pos >= s.length()) throw new IllegalArgumentException("unterminated escape");
                        char e = s.charAt(pos++);
                        switch (e) {
                            case '"': sb.append('"'); break;
                            case '\\': sb.append('\\'); break;
                            case '/': sb.append('/'); break;
                            case 'b': sb.append('\b'); break;
                            case 'f': sb.append('\f'); break;
                            case 'n': sb.append('\n'); break;
                            case 'r': sb.append('\r'); break;
                            case 't': sb.append('\t'); break;
                            case 'u':
                                if (pos + 4 > s.length()) throw new IllegalArgumentException("bad \\u escape");
                                sb.append((char) Integer.parseInt(s.substring(pos, pos + 4), 16));
                                pos += 4;
                                break;
                            default: throw new IllegalArgumentException("bad escape '\\" + e + "'");
                        }
                    } else {
                        sb.append(c);
                    }
                }
            }

            Object number() {
                int start = pos;
                if (pos < s.length() && s.charAt(pos) == '-') pos++;
                boolean isDouble = false;
                while (pos < s.length()) {
                    char c = s.charAt(pos);
                    if (c >= '0' && c <= '9') { pos++; }
                    else if (c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') { isDouble = isDouble || c == '.' || c == 'e' || c == 'E'; pos++; }
                    else break;
                }
                String tok = s.substring(start, pos);
                if (tok.isEmpty() || "-".equals(tok)) throw new IllegalArgumentException("invalid number at " + start);
                try {
                    return isDouble ? (Object) Double.parseDouble(tok) : (Object) Long.parseLong(tok);
                } catch (NumberFormatException e) {
                    try {
                        return Double.parseDouble(tok);
                    } catch (NumberFormatException e2) {
                        throw new IllegalArgumentException("invalid number '" + tok + "'");
                    }
                }
            }
        }

        static String write(Object o) {
            StringBuilder sb = new StringBuilder(256);
            writeTo(o, sb);
            return sb.toString();
        }

        @SuppressWarnings("unchecked")
        static void writeTo(Object o, StringBuilder sb) {
            if (o == null) {
                sb.append("null");
            } else if (o instanceof String) {
                writeString((String) o, sb);
            } else if (o instanceof Double || o instanceof Float) {
                double d = ((Number) o).doubleValue();
                if (Double.isNaN(d) || Double.isInfinite(d)) throw new IllegalArgumentException("non-finite double");
                sb.append(Double.toString(d)); // shortest round-trip; integral keeps ".0"
            } else if (o instanceof Number) {
                sb.append(o.toString());
            } else if (o instanceof Boolean) {
                sb.append(o.toString());
            } else if (o instanceof Map) {
                sb.append('{');
                boolean first = true;
                for (Map.Entry<String, Object> e : ((Map<String, Object>) o).entrySet()) {
                    if (!first) sb.append(',');
                    first = false;
                    writeString(e.getKey(), sb);
                    sb.append(':');
                    writeTo(e.getValue(), sb);
                }
                sb.append('}');
            } else if (o instanceof List) {
                sb.append('[');
                boolean first = true;
                for (Object e : (List<Object>) o) {
                    if (!first) sb.append(',');
                    first = false;
                    writeTo(e, sb);
                }
                sb.append(']');
            } else {
                throw new IllegalArgumentException("cannot serialize " + o.getClass());
            }
        }

        static void writeString(String s, StringBuilder sb) {
            sb.append('"');
            for (int i = 0; i < s.length(); i++) {
                char c = s.charAt(i);
                switch (c) {
                    case '"': sb.append("\\\""); break;
                    case '\\': sb.append("\\\\"); break;
                    case '\b': sb.append("\\b"); break;
                    case '\f': sb.append("\\f"); break;
                    case '\n': sb.append("\\n"); break;
                    case '\r': sb.append("\\r"); break;
                    case '\t': sb.append("\\t"); break;
                    default:
                        if (c < 0x20) {
                            sb.append(String.format("\\u%04x", (int) c));
                        } else {
                            sb.append(c);
                        }
                }
            }
            sb.append('"');
        }
    }

    // ── Shared benchmark dataset (API-SPEC.md §2) — load failure is FATAL ─

    /** Parsed user objects from bench-data.json (LinkedHashMap each). */
    private static List<Object> benchUsers;
    /** Search corpus strings. */
    private static List<String> benchSearchCorpus;
    /** Timeseries `value` fields, in dataset order (audit F6: parsed once). */
    private static double[] benchTsValues;
    /** Expected checksums object, echoed by /api/validate. */
    private static Map<String, Object> benchChecksums;

    private static void fatal(String msg) {
        System.err.println("FATAL: " + msg);
        System.exit(1);
    }

    @SuppressWarnings("unchecked")
    private static void loadBenchData() {
        String chosen = null;
        String envPath = System.getenv("BENCH_DATA_PATH");
        if (envPath != null && !envPath.isEmpty()) {
            chosen = envPath; // must exist and parse — no fallback
        } else {
            String[] candidates = {
                "/opt/bench/bench-data.json",
                Path.of("../shared/bench-data.json").toAbsolutePath().normalize().toString(),
            };
            for (String c : candidates) {
                if (Files.exists(Path.of(c))) { chosen = c; break; } // first existing wins
            }
            if (chosen == null) {
                fatal("bench-data.json not found (tried BENCH_DATA_PATH, /opt/bench/bench-data.json, "
                    + "../shared/bench-data.json); the shared dataset is required — there is no PRNG fallback");
            }
        }

        Map<String, Object> data;
        try {
            String content = Files.readString(Path.of(chosen), StandardCharsets.UTF_8);
            data = (Map<String, Object>) Json.parse(content);
        } catch (Exception e) {
            fatal("bench-data.json at " + chosen + " could not be loaded: " + e.getMessage());
            return; // unreachable
        }

        Object version = data.get("_version");
        if (!(version instanceof Long) || (Long) version != 2L) {
            fatal("bench-data.json at " + chosen + ": _version=" + version + ", want 2");
        }
        List<Object> users = (List<Object>) data.get("users");
        List<Object> corpus = (List<Object>) data.get("search_corpus");
        List<Object> timeseries = (List<Object>) data.get("timeseries");
        List<Object> transformInputs = (List<Object>) data.get("transform_inputs");
        Map<String, Object> checksums = (Map<String, Object>) data.get("expected_checksums");
        if (users == null || users.size() != 100) fatal("bench-data.json: users count != 100");
        if (corpus == null || corpus.size() != 1000) fatal("bench-data.json: search_corpus count != 1000");
        if (timeseries == null || timeseries.size() != 10000) fatal("bench-data.json: timeseries count != 10000");
        if (transformInputs == null || transformInputs.size() != 10) fatal("bench-data.json: transform_inputs count != 10");
        if (checksums == null || checksums.size() != 4) fatal("bench-data.json: expected_checksums keys != 4");

        benchUsers = users;
        benchSearchCorpus = new ArrayList<>(corpus.size());
        for (Object o : corpus) benchSearchCorpus.add((String) o);
        benchTsValues = new double[timeseries.size()];
        for (int i = 0; i < timeseries.size(); i++) {
            // §2: timeseries entries are OBJECTS {ts, value, category} —
            // read the `value` field (audit F2).
            Map<String, Object> point = (Map<String, Object>) timeseries.get(i);
            benchTsValues[i] = ((Number) point.get("value")).doubleValue();
        }
        benchChecksums = checksums;

        logger.info(String.format("Loaded bench-data.json from %s (%d users, %d corpus, %d timeseries)",
                chosen, benchUsers.size(), benchSearchCorpus.size(), benchTsValues.length));
    }

    // ── Constant-work /health body (§5.1) — precomputed once at startup ─

    private static final byte[] HEALTH_BODY = String.format(
            "{\"status\":\"ok\",\"runtime\":\"java\",\"version\":\"%s\"}",
            System.getProperty("java.version")
    ).getBytes(StandardCharsets.UTF_8);

    /** Shared 8 KiB chunk of 0x42 for /download (§5.2). */
    private static final byte[] FILL_CHUNK = new byte[CHUNK_SIZE];
    static {
        java.util.Arrays.fill(FILL_CHUNK, (byte) 0x42);
    }

    /** Timer for /api/delayed — the sleep must not block a pool worker (§5.9). */
    private static final ScheduledExecutorService SCHEDULER =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "delayed-timer");
                t.setDaemon(true);
                return t;
            });

    public static void main(String[] args) throws Exception {
        loadBenchData();

        // Worker policy (§3): BENCH_WORKERS maps to the HttpServer executor's
        // fixed thread-pool size, default = logical CPU count.
        int nproc = Runtime.getRuntime().availableProcessors();
        int workers = nproc;
        String w = System.getenv("BENCH_WORKERS");
        if (w != null && !w.isEmpty()) {
            try {
                workers = Integer.parseInt(w);
            } catch (NumberFormatException e) {
                fatal("BENCH_WORKERS=" + w + " is not a positive integer");
            }
            if (workers < 1) fatal("BENCH_WORKERS=" + w + " is not a positive integer");
        }

        // Check if TLS certs exist — if not, run plain HTTP (application mode)
        boolean useTls = Files.exists(Path.of(CERT_DIR, "cert.pem"))
                      && Files.exists(Path.of(CERT_DIR, "key.pem"));

        com.sun.net.httpserver.HttpServer server;
        if (useTls) {
            SSLContext sslContext = buildSslContext(
                    Path.of(CERT_DIR, "cert.pem"),
                    Path.of(CERT_DIR, "key.pem")
            );
            HttpsServer httpsServer = HttpsServer.create(new InetSocketAddress(PORT), 0);
            httpsServer.setHttpsConfigurator(new HttpsConfigurator(sslContext) {
                @Override
                public void configure(HttpsParameters params) {
                    SSLParameters sslParams = sslContext.getDefaultSSLParameters();
                    params.setSSLParameters(sslParams);
                }
            });
            server = httpsServer;
        } else {
            server = com.sun.net.httpserver.HttpServer.create(new InetSocketAddress(PORT), 0);
        }

        server.createContext("/health", new HealthHandler());
        server.createContext("/download/", new DownloadHandler());
        server.createContext("/upload", new UploadHandler());
        server.createContext("/api/users", new APIUsersHandler());
        server.createContext("/api/transform", new APITransformHandler());
        server.createContext("/api/aggregate", new APIAggregateHandler());
        server.createContext("/api/search", new APISearchHandler());
        server.createContext("/api/upload/process", new APIUploadProcessHandler());
        server.createContext("/api/delayed", new APIDelayedHandler());
        server.createContext("/api/validate", new APIValidateHandler());

        server.setExecutor(Executors.newFixedThreadPool(workers));
        server.start();

        logger.info(String.format("Java %s server listening on :%d (nproc=%d bench_workers=%d, fixed thread pool)",
                useTls ? "HTTPS" : "HTTP", PORT, nproc, workers));
    }

    // ── Auth (§1) ─────────────────────────────────────────────────────

    /**
     * Check bearer token auth. Returns true if authorized, false if 401 was sent.
     * /health is always exempt. If BENCH_API_TOKEN is unset, auth is disabled.
     */
    static boolean checkAuth(HttpExchange exchange) throws IOException {
        if (BENCH_TOKEN == null || BENCH_TOKEN.isEmpty()) return true;
        String path = exchange.getRequestURI().getPath();
        if ("/health".equals(path)) return true;
        String auth = exchange.getRequestHeaders().getFirst("Authorization");
        if (auth != null && auth.equals("Bearer " + BENCH_TOKEN)) return true;
        byte[] body = "{\"error\":\"unauthorized\"}".getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", "application/json");
        exchange.sendResponseHeaders(401, body.length);
        try (OutputStream out = exchange.getResponseBody()) {
            out.write(body);
        }
        return false;
    }

    // ── Response helpers ──────────────────────────────────────────────

    /** Set the §1 benchmark headers; return start time in nanos. */
    private static long setAPIHeaders(HttpExchange ex) {
        ex.getResponseHeaders().set("Content-Type", "application/json");
        ex.getResponseHeaders().set("Cache-Control", "no-store, no-cache, must-revalidate");
        ex.getResponseHeaders().set("Timing-Allow-Origin", "*");
        ex.getResponseHeaders().set("Access-Control-Allow-Origin", "*");
        return System.nanoTime();
    }

    /** True when the request should be served like GET with the body
     *  suppressed (the validator checks §1 headers with `curl -I`). */
    private static boolean isHead(HttpExchange ex) {
        return "HEAD".equals(ex.getRequestMethod());
    }

    /** True when the method is neither GET nor HEAD. */
    private static boolean notGet(HttpExchange ex) {
        String m = ex.getRequestMethod();
        return !"GET".equals(m) && !"HEAD".equals(m);
    }

    /** Send a JSON response with Server-Timing (works for success AND errors,
     *  so §10 item 7 — bench headers on all /api/* responses — holds).
     *  HEAD requests get headers only. */
    private static void sendAPI(HttpExchange ex, long startNanos, int status, String json) throws IOException {
        double dur = (System.nanoTime() - startNanos) / 1_000_000.0;
        ex.getResponseHeaders().set("Server-Timing", String.format("app;dur=%.1f", dur));
        byte[] bytes = json.getBytes(StandardCharsets.UTF_8);
        if (isHead(ex) || bytes.length == 0) {
            ex.sendResponseHeaders(status, -1);
            ex.close();
            return;
        }
        ex.sendResponseHeaders(status, bytes.length);
        try (OutputStream out = ex.getResponseBody()) {
            out.write(bytes);
        }
    }

    private static void sendAPIError(HttpExchange ex, long startNanos, int status, String message) throws IOException {
        Map<String, Object> err = new LinkedHashMap<>();
        err.put("error", message);
        sendAPI(ex, startNanos, status, Json.write(err));
    }

    /** Plain JSON send for non-/api routes. */
    private static void sendText(HttpExchange ex, int code, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        ex.getResponseHeaders().set("Content-Type", "application/json");
        ex.sendResponseHeaders(code, bytes.length);
        try (OutputStream out = ex.getResponseBody()) {
            out.write(bytes);
        }
    }

    /** Parse query string into a map. */
    private static Map<String, String> parseQuery(String query) {
        Map<String, String> params = new HashMap<>();
        if (query == null || query.isEmpty()) return params;
        for (String pair : query.split("&")) {
            int eq = pair.indexOf('=');
            if (eq > 0) {
                String key = URLDecoder.decode(pair.substring(0, eq), StandardCharsets.UTF_8);
                String val = URLDecoder.decode(pair.substring(eq + 1), StandardCharsets.UTF_8);
                params.put(key, val);
            }
        }
        return params;
    }

    /** Hex-encode bytes. */
    private static String hexEncode(byte[] bytes) {
        StringBuilder sb = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            sb.append(String.format("%02x", b & 0xff));
        }
        return sb.toString();
    }

    private static String sha256Hex(byte[] input) {
        try {
            MessageDigest md = MessageDigest.getInstance("SHA-256");
            return hexEncode(md.digest(input));
        } catch (Exception e) {
            throw new RuntimeException(e); // SHA-256 is always available
        }
    }

    /** r2: §5.6 rounding — half away from zero to 2 decimals. */
    private static double r2(double x) {
        return Math.floor(x * 100.0 + 0.5) / 100.0;
    }

    // ── Handlers ──────────────────────────────────────────────────────

    // GET /health — byte-constant body precomputed at startup (§5.1).
    static class HealthHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            ex.getResponseHeaders().set("Content-Type", "application/json");
            if (isHead(ex)) {
                ex.sendResponseHeaders(200, -1);
                ex.close();
                return;
            }
            ex.sendResponseHeaders(200, HEALTH_BODY.length);
            try (OutputStream out = ex.getResponseBody()) {
                out.write(HEALTH_BODY);
            }
        }
    }

    // GET /download/{size} — 0x42 fill, 8 KiB chunks, 2 GiB clamp (§5.2).
    static class DownloadHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = System.nanoTime();

            String path = ex.getRequestURI().getPath();   // /download/1048576
            String sizeStr = path.substring(path.lastIndexOf('/') + 1);
            long size;
            try {
                size = Long.parseLong(sizeStr);
            } catch (NumberFormatException e) {
                sendText(ex, 400, "{\"error\":\"invalid size\"}");
                return;
            }
            if (size < 0) {
                sendText(ex, 400, "{\"error\":\"invalid size\"}");
                return;
            }
            if (size > MAX_DOWNLOAD) size = MAX_DOWNLOAD; // clamp, not reject

            double procMs = (System.nanoTime() - start) / 1_000_000.0;
            ex.getResponseHeaders().set("Content-Type", "application/octet-stream");
            ex.getResponseHeaders().set("X-Download-Bytes", Long.toString(size));
            ex.getResponseHeaders().set("Server-Timing", String.format("proc;dur=%.1f", procMs));
            // sendResponseHeaders: length 0 means chunked; -1 means no body.
            if (isHead(ex) || size == 0) {
                ex.sendResponseHeaders(200, -1);
                ex.close();
                return;
            }
            ex.sendResponseHeaders(200, size);
            try (OutputStream out = ex.getResponseBody()) {
                long remaining = size;
                while (remaining > 0) {
                    int toWrite = (int) Math.min(remaining, CHUNK_SIZE);
                    out.write(FILL_CHUNK, 0, toWrite);
                    remaining -= toWrite;
                }
            }
        }
    }

    // POST /upload — drain body without wholesale buffering (§5.3).
    static class UploadHandler implements HttpHandler {
        private static final int BUF_SIZE = 64 * 1024;

        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = System.nanoTime();

            long received = 0;
            byte[] buf = new byte[BUF_SIZE];
            try (InputStream in = ex.getRequestBody()) {
                int n;
                while ((n = in.read(buf)) != -1) {
                    received += n;
                }
            }

            double recvMs = (System.nanoTime() - start) / 1_000_000.0;
            ex.getResponseHeaders().set("Content-Type", "application/json");
            ex.getResponseHeaders().set("X-Networker-Received-Bytes", Long.toString(received));
            ex.getResponseHeaders().set("Server-Timing", String.format("recv;dur=%.1f", recvMs));
            String reqId = ex.getRequestHeaders().getFirst("X-Networker-Request-Id");
            if (reqId != null) {
                ex.getResponseHeaders().set("X-Networker-Request-Id", reqId);
            }
            byte[] body = String.format("{\"received_bytes\":%d}", received).getBytes(StandardCharsets.UTF_8);
            ex.sendResponseHeaders(200, body.length);
            try (OutputStream out = ex.getResponseBody()) {
                out.write(body);
            }
        }
    }

    // ── JSON API Handlers (family C) ──────────────────────────────────

    // GET /api/users?page=N&sort=<field>&order=<asc|desc> (§5.4).
    static class APIUsersHandler implements HttpHandler {
        @Override
        @SuppressWarnings("unchecked")
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            int page = 1;
            try { page = Integer.parseInt(params.getOrDefault("page", "1")); } catch (NumberFormatException ignored) {}
            if (page < 1) page = 1;

            String sortField = params.getOrDefault("sort", "id");
            boolean desc = "desc".equals(params.getOrDefault("order", "asc"));

            // 100-user window; the dataset has 100 users, so page ≥ 2 is [].
            int winStart = (page - 1) * 100;
            int winEnd = Math.min(winStart + 100, benchUsers.size());
            List<Object> window = new ArrayList<>();
            if (winStart < benchUsers.size()) {
                window.addAll(benchUsers.subList(winStart, winEnd));
            }

            Comparator<Object> cmp;
            switch (sortField) {
                case "name":
                case "email":
                case "created_at":
                    final String sf = sortField;
                    cmp = Comparator.comparing(u -> (String) ((Map<String, Object>) u).get(sf));
                    break;
                case "score":
                    cmp = Comparator.comparingDouble(
                            u -> ((Number) ((Map<String, Object>) u).get("score")).doubleValue());
                    break;
                default: // "id" and any unrecognized value
                    cmp = Comparator.comparingLong(
                            u -> ((Number) ((Map<String, Object>) u).get("id")).longValue());
                    break;
            }
            // List.sort is stable (TimSort); desc reverses the comparator so
            // ties keep dataset order.
            window.sort(desc ? cmp.reversed() : cmp);

            List<Object> result = window.size() > 20 ? window.subList(0, 20) : window;
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // POST /api/transform — SHA-256 each field, reverse values (§5.5).
    static class APITransformHandler implements HttpHandler {
        @Override
        @SuppressWarnings("unchecked")
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);

            String bodyStr;
            try (InputStream in = ex.getRequestBody()) {
                bodyStr = new String(in.readAllBytes(), StandardCharsets.UTF_8);
            }

            Map<String, Object> body;
            try {
                Object parsed = Json.parse(bodyStr);
                if (!(parsed instanceof Map)) throw new IllegalArgumentException("not an object");
                body = (Map<String, Object>) parsed;
            } catch (Exception e) {
                sendAPIError(ex, start, 400, "invalid JSON");
                return;
            }

            Object seed = body.get("seed");
            if (!(seed instanceof Number)) seed = 0L;
            List<Object> fields = body.get("fields") instanceof List ? (List<Object>) body.get("fields") : List.of();
            List<Object> values = body.get("values") instanceof List ? (List<Object>) body.get("values") : List.of();

            List<Object> hashedFields = new ArrayList<>(fields.size());
            for (Object f : fields) {
                hashedFields.add(sha256Hex(String.valueOf(f).getBytes(StandardCharsets.UTF_8)));
            }
            List<Object> reversedValues = new ArrayList<>(values);
            java.util.Collections.reverse(reversedValues);

            Map<String, Object> result = new LinkedHashMap<>();
            result.put("seed", seed);
            result.put("hashed_fields", hashedFields);
            result.put("reversed_values", reversedValues);
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // GET /api/aggregate[?range=start,end] — range accepted and IGNORED (§5.6).
    static class APIAggregateHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);

            double[] values = benchTsValues.clone();
            java.util.Arrays.sort(values);
            int n = values.length;
            double sum = 0.0;
            for (double v : values) sum += v; // sequential sum of SORTED values

            int chunk = n / 5;
            List<Object> categories = new ArrayList<>(5);
            for (int i = 0; i < 5; i++) {
                double partSum = 0.0;
                for (int j = i * chunk; j < (i + 1) * chunk; j++) partSum += values[j];
                Map<String, Object> cat = new LinkedHashMap<>();
                cat.put("category", "q" + (i + 1));
                cat.put("count", (long) chunk);
                cat.put("mean", r2(partSum / chunk));
                cat.put("min", r2(values[i * chunk]));
                cat.put("max", r2(values[(i + 1) * chunk - 1]));
                categories.add(cat);
            }

            Map<String, Object> result = new LinkedHashMap<>();
            result.put("total_points", (long) n);
            result.put("mean", r2(sum / n));
            result.put("p50", r2(values[(int) (n * 0.50)]));
            result.put("p95", r2(values[(int) (n * 0.95)]));
            result.put("max", r2(values[n - 1]));
            result.put("categories", categories);
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // GET /api/search?q=<term>&limit=N — case-sensitive regex with literal
    // fallback on compile failure (§5.7).
    static class APISearchHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            String q = params.getOrDefault("q", "test");
            if (q.isEmpty()) q = "test";
            int limit = 20;
            try { limit = Integer.parseInt(params.getOrDefault("limit", "20")); } catch (NumberFormatException ignored) {}
            if (limit > 100) limit = 100;
            if (limit < 0) limit = 0;

            Pattern pattern = null;
            try {
                pattern = Pattern.compile(q); // raw, case-sensitive
            } catch (PatternSyntaxException ignored) {
                // literal-substring fallback
            }

            List<int[]> positions = new ArrayList<>();   // parallel to matchItems
            List<String> matchItems = new ArrayList<>();
            for (String item : benchSearchCorpus) {
                int pos = -1;
                if (pattern != null) {
                    Matcher m = pattern.matcher(item);
                    if (m.find()) pos = m.start();
                } else {
                    pos = item.indexOf(q);
                }
                if (pos >= 0) {
                    positions.add(new int[]{pos});
                    matchItems.add(item);
                }
            }

            // Sort by (position asc, item asc bytewise).
            Integer[] order = new Integer[matchItems.size()];
            for (int i = 0; i < order.length; i++) order[i] = i;
            java.util.Arrays.sort(order, (a, b) -> {
                int c = Integer.compare(positions.get(a)[0], positions.get(b)[0]);
                if (c != 0) return c;
                return matchItems.get(a).compareTo(matchItems.get(b));
            });

            int total = matchItems.size();
            int returned = Math.min(limit, total);
            List<Object> results = new ArrayList<>(returned);
            for (int i = 0; i < returned; i++) {
                Map<String, Object> r = new LinkedHashMap<>();
                r.put("rank", (long) (i + 1));
                r.put("item", matchItems.get(order[i]));
                r.put("match_position", (long) positions.get(order[i])[0]);
                results.add(r);
            }

            Map<String, Object> result = new LinkedHashMap<>();
            result.put("query", q);
            result.put("total_matches", (long) total);
            result.put("returned", (long) returned);
            result.put("results", results);
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // POST /api/upload/process — CRC-32 + SHA-256 + zlib level 6 (§5.8).
    static class APIUploadProcessHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);

            byte[] body;
            try (InputStream in = ex.getRequestBody()) {
                body = in.readAllBytes();
            }

            // CRC-32 (IEEE), 8 lowercase hex chars, zero-padded.
            CRC32 crc = new CRC32();
            crc.update(body);
            String crcHex = String.format("%08x", crc.getValue());

            String shaHex = sha256Hex(body);

            // zlib (RFC 1950, with header/adler) at level 6 — NOT raw deflate.
            Deflater deflater = new Deflater(6);
            deflater.setInput(body);
            deflater.finish();
            ByteArrayOutputStream compressed = new ByteArrayOutputStream();
            byte[] buf = new byte[8192];
            while (!deflater.finished()) {
                int n = deflater.deflate(buf);
                compressed.write(buf, 0, n);
            }
            deflater.end();

            Map<String, Object> result = new LinkedHashMap<>();
            result.put("original_size", (long) body.length);
            result.put("compressed_size", (long) compressed.size());
            result.put("crc32", crcHex);
            result.put("sha256", shaHex);
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // GET /api/delayed?ms=N&work=<ignored> — async timer delay (§5.9).
    // The response is completed from the scheduler thread so the sleep never
    // blocks a fixed-pool worker.
    static class APIDelayedHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            int msParam = 10;
            try { msParam = Integer.parseInt(params.getOrDefault("ms", "10")); } catch (NumberFormatException ignored) {}
            final int ms = Math.max(1, Math.min(100, msParam));
            // `work` is reserved: accepted and ignored.

            SCHEDULER.schedule(() -> {
                try {
                    double actualMs = Math.round((System.nanoTime() - start) / 10_000.0) / 100.0;
                    Map<String, Object> result = new LinkedHashMap<>();
                    result.put("requested_ms", (long) ms);
                    result.put("actual_ms", actualMs);
                    sendAPI(ex, start, 200, Json.write(result));
                } catch (IOException e) {
                    logger.fine("delayed response failed: " + e.getMessage());
                    ex.close();
                }
            }, ms, TimeUnit.MILLISECONDS);
            // handle() returns without closing the exchange; the scheduler
            // completes it after the delay.
        }
    }

    // GET /api/validate?seed=N — echo the dataset's expected_checksums (§5.10).
    static class APIValidateHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!checkAuth(ex)) return;
            if (notGet(ex)) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            long seed = 42;
            try { seed = Long.parseLong(params.getOrDefault("seed", "42")); } catch (NumberFormatException ignored) {}

            Map<String, Object> result = new LinkedHashMap<>();
            result.put("seed", seed);
            result.put("checksums", benchChecksums);
            sendAPI(ex, start, 200, Json.write(result));
        }
    }

    // ── TLS helpers ───────────────────────────────────────────────────

    /**
     * Build an SSLContext from PEM-encoded certificate and PKCS#8 private key.
     * Works with the output of generate-cert.sh (openssl req -x509 -newkey rsa:2048).
     */
    private static SSLContext buildSslContext(Path certPath, Path keyPath) throws Exception {
        // Parse certificate
        CertificateFactory cf = CertificateFactory.getInstance("X.509");
        Certificate cert;
        try (InputStream is = new ByteArrayInputStream(Files.readAllBytes(certPath))) {
            cert = cf.generateCertificate(is);
        }

        // Parse PKCS#8 PEM private key
        String keyPem = Files.readString(keyPath, StandardCharsets.UTF_8);
        String keyBase64 = keyPem
                .replace("-----BEGIN PRIVATE KEY-----", "")
                .replace("-----END PRIVATE KEY-----", "")
                .replaceAll("\\s+", "");
        byte[] keyBytes = Base64.getDecoder().decode(keyBase64);
        PrivateKey privateKey = KeyFactory.getInstance("RSA")
                .generatePrivate(new PKCS8EncodedKeySpec(keyBytes));

        // Build KeyStore
        KeyStore ks = KeyStore.getInstance("PKCS12");
        ks.load(null, null);
        ks.setKeyEntry("server", privateKey, new char[0], new Certificate[]{cert});

        KeyManagerFactory kmf = KeyManagerFactory.getInstance(
                KeyManagerFactory.getDefaultAlgorithm());
        kmf.init(ks, new char[0]);

        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(kmf.getKeyManagers(), null, null);
        return ctx;
    }
}
