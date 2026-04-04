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
import java.time.Instant;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.concurrent.Executors;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.zip.CRC32;
import java.util.zip.Deflater;

/**
 * AletheBench Java reference API.
 *
 * Single-file HTTPS server using JDK built-in com.sun.net.httpserver with
 * Virtual Threads (Java 21+). No external dependencies.
 *
 * Endpoints:
 *   GET  /health                -> {"status":"ok","language":"java","runtime":"..."}
 *   GET  /download/{size}       -> stream `size` bytes of zeros
 *   POST /upload                -> read body, return {"bytes_received": N}
 *   GET  /api/users             -> paginated sorted user list
 *   POST /api/transform         -> hash strings, reverse arrays
 *   GET  /api/aggregate         -> statistics over generated data points
 *   GET  /api/search            -> regex search over generated strings
 *   POST /api/upload/process    -> hash and compress uploaded body
 *   GET  /api/delayed           -> sleep with optional CPU work
 *   GET  /api/validate          -> checksums for all endpoints
 */
public class Server {

    private static final int PORT = 8443;
    private static final String CERT_DIR = System.getenv().getOrDefault("BENCH_CERT_DIR", "/opt/bench");

    public static void main(String[] args) throws Exception {
        SSLContext sslContext = buildSslContext(
                Path.of(CERT_DIR, "cert.pem"),
                Path.of(CERT_DIR, "key.pem")
        );

        HttpsServer server = HttpsServer.create(new InetSocketAddress(PORT), 0);
        server.setHttpsConfigurator(new HttpsConfigurator(sslContext) {
            @Override
            public void configure(HttpsParameters params) {
                SSLParameters sslParams = sslContext.getDefaultSSLParameters();
                params.setSSLParameters(sslParams);
            }
        });

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

        server.setExecutor(Executors.newVirtualThreadPerTaskExecutor());
        server.start();

        System.out.printf("Java HTTPS server listening on :%d (Virtual Threads)%n", PORT);
    }

    // ── Handlers ──────────────────────────────────────────────────────

    static class HealthHandler implements HttpHandler {
        private static final String BODY = String.format(
                "{\"status\":\"ok\",\"language\":\"java\",\"runtime\":\"%s\"}",
                System.getProperty("java.version")
        );

        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            sendText(ex, 200, BODY);
        }
    }

    static class DownloadHandler implements HttpHandler {
        private static final int CHUNK = 64 * 1024;
        private static final byte[] ZEROS = new byte[CHUNK];

        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }

            String path = ex.getRequestURI().getPath();   // /download/1048576
            String sizeStr = path.substring(path.lastIndexOf('/') + 1);
            long size;
            try {
                size = Long.parseLong(sizeStr);
            } catch (NumberFormatException e) {
                sendText(ex, 400, "{\"error\":\"invalid size\"}");
                return;
            }
            if (size < 0 || size > 1_073_741_824L) {
                sendText(ex, 400, "{\"error\":\"size must be 0..1GiB\"}");
                return;
            }

            ex.getResponseHeaders().set("Content-Type", "application/octet-stream");
            ex.sendResponseHeaders(200, size);
            try (OutputStream out = ex.getResponseBody()) {
                long remaining = size;
                while (remaining > 0) {
                    int toWrite = (int) Math.min(remaining, CHUNK);
                    out.write(ZEROS, 0, toWrite);
                    remaining -= toWrite;
                }
            }
        }
    }

    static class UploadHandler implements HttpHandler {
        private static final int BUF_SIZE = 64 * 1024;

        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }

            long received = 0;
            byte[] buf = new byte[BUF_SIZE];
            try (InputStream in = ex.getRequestBody()) {
                int n;
                while ((n = in.read(buf)) != -1) {
                    received += n;
                }
            }

            String body = String.format("{\"bytes_received\":%d}", received);
            sendText(ex, 200, body);
        }
    }

    // ── JSON API Handlers ─────────────────────────────────────────────

    private static final String[] FIRST_NAMES = {
            "Alice", "Bob", "Carol", "Dave", "Eve", "Frank", "Grace", "Hank",
            "Ivy", "Jack", "Kara", "Leo", "Mia", "Nick", "Olga", "Paul",
            "Quinn", "Rita", "Sam", "Tina"
    };
    private static final String[] LAST_NAMES = {
            "Smith", "Johnson", "Brown", "Taylor", "Anderson", "Thomas", "Jackson",
            "White", "Harris", "Martin", "Garcia", "Clark", "Lewis", "Hall", "Young",
            "King", "Wright", "Lopez", "Hill", "Scott"
    };
    private static final String[] DOMAINS = {"example.com", "test.org", "demo.net", "bench.io", "sample.dev"};
    private static final String[] WORDS = {
            "network", "latency", "throughput", "bandwidth", "packet", "socket",
            "connection", "timeout", "buffer", "stream", "protocol", "endpoint",
            "request", "response", "header", "payload", "router", "gateway",
            "firewall", "proxy"
    };
    private static final String[] CAT_NAMES = {"alpha", "beta", "gamma", "delta", "epsilon"};

    /** Set common benchmark headers; return start time in nanos. */
    private static long setAPIHeaders(HttpExchange ex) {
        ex.getResponseHeaders().set("Content-Type", "application/json");
        ex.getResponseHeaders().set("Cache-Control", "no-store, no-cache, must-revalidate");
        ex.getResponseHeaders().set("Timing-Allow-Origin", "*");
        ex.getResponseHeaders().set("Access-Control-Allow-Origin", "*");
        return System.nanoTime();
    }

    /** Write Server-Timing header from start nanos. */
    private static void writeServerTiming(HttpExchange ex, long startNanos) {
        double dur = (System.nanoTime() - startNanos) / 1_000_000.0;
        ex.getResponseHeaders().set("Server-Timing", String.format("app;dur=%.1f", dur));
    }

    /** Send JSON response with Server-Timing. */
    private static void sendAPI(HttpExchange ex, long startNanos, String json) throws IOException {
        writeServerTiming(ex, startNanos);
        byte[] bytes = json.getBytes(StandardCharsets.UTF_8);
        ex.sendResponseHeaders(200, bytes.length);
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

    /** Generate 100 users from seed. */
    private static List<Map<String, Object>> generateUsers(long seed) {
        Random rng = new Random(seed);
        List<Map<String, Object>> users = new ArrayList<>(100);
        for (int i = 0; i < 100; i++) {
            String first = FIRST_NAMES[rng.nextInt(FIRST_NAMES.length)];
            String last = LAST_NAMES[rng.nextInt(LAST_NAMES.length)];
            String domain = DOMAINS[rng.nextInt(DOMAINS.length)];

            Map<String, Object> user = new LinkedHashMap<>();
            user.put("id", i + 1);
            user.put("name", first + " " + last);
            user.put("email", first.toLowerCase() + "." + last.toLowerCase() + "@" + domain);
            user.put("age", 20 + rng.nextInt(50));
            user.put("score", rng.nextInt(1000));
            user.put("active", rng.nextInt(2) == 1);
            user.put("created_at", String.format("2025-%02d-%02d", 1 + rng.nextInt(12), 1 + rng.nextInt(28)));
            users.add(user);
        }
        return users;
    }

    /** Escape a string for JSON output. */
    private static String jsonEscape(String s) {
        return s.replace("\\", "\\\\")
                .replace("\"", "\\\"")
                .replace("\n", "\\n")
                .replace("\r", "\\r")
                .replace("\t", "\\t");
    }

    /** Serialize a user map to JSON object string. */
    private static String userToJson(Map<String, Object> user) {
        return String.format(
                "{\"id\":%d,\"name\":\"%s\",\"email\":\"%s\",\"age\":%d,\"score\":%d,\"active\":%s,\"created_at\":\"%s\"}",
                user.get("id"), jsonEscape((String) user.get("name")),
                jsonEscape((String) user.get("email")), user.get("age"),
                user.get("score"), user.get("active"), user.get("created_at")
        );
    }

    /** Serialize a list of user maps to JSON array. */
    private static String usersToJson(List<Map<String, Object>> users) {
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < users.size(); i++) {
            if (i > 0) sb.append(",");
            sb.append(userToJson(users.get(i)));
        }
        sb.append("]");
        return sb.toString();
    }

    // GET /api/users?page=N&sort=field&order=asc — paginated sorted user list.
    static class APIUsersHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            int page = 1;
            try { page = Integer.parseInt(params.getOrDefault("page", "1")); } catch (NumberFormatException ignored) {}
            if (page < 1) page = 1;

            String sortField = params.getOrDefault("sort", "");
            String order = params.getOrDefault("order", "");

            List<Map<String, Object>> users = generateUsers(page);

            Comparator<Map<String, Object>> cmp;
            switch (sortField) {
                case "name":
                    cmp = Comparator.comparing(u -> (String) u.get("name"));
                    break;
                case "email":
                    cmp = Comparator.comparing(u -> (String) u.get("email"));
                    break;
                case "age":
                    cmp = Comparator.comparingInt(u -> (Integer) u.get("age"));
                    break;
                case "score":
                    cmp = Comparator.comparingInt(u -> (Integer) u.get("score"));
                    break;
                default:
                    cmp = Comparator.comparingInt(u -> (Integer) u.get("id"));
                    break;
            }
            users.sort(cmp);
            if ("desc".equals(order)) {
                Collections.reverse(users);
            }

            int pageSize = 20;
            int offset = (page - 1) * pageSize;
            if (offset > users.size()) offset = users.size();
            int end = offset + pageSize;
            if (end > users.size()) end = users.size();
            List<Map<String, Object>> result = users.subList(offset, end);

            sendAPI(ex, start, usersToJson(result));
        }
    }

    // POST /api/transform — hash string fields, reverse arrays.
    static class APITransformHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);

            String body;
            try (InputStream in = ex.getRequestBody()) {
                body = new String(in.readAllBytes(), StandardCharsets.UTF_8);
            }

            // Minimal JSON parsing: top-level object with string or array values.
            // Strip outer braces, split on top-level commas.
            body = body.trim();
            if (!body.startsWith("{") || !body.endsWith("}")) {
                sendText(ex, 400, "{\"error\":\"invalid JSON\"}");
                return;
            }
            body = body.substring(1, body.length() - 1).trim();

            StringBuilder result = new StringBuilder("{");
            boolean first = true;

            // Parse key-value pairs at top level
            int pos = 0;
            while (pos < body.length()) {
                // Skip whitespace
                while (pos < body.length() && Character.isWhitespace(body.charAt(pos))) pos++;
                if (pos >= body.length()) break;

                // Parse key (quoted string)
                if (body.charAt(pos) != '"') break;
                int keyStart = pos + 1;
                int keyEnd = body.indexOf('"', keyStart);
                String key = body.substring(keyStart, keyEnd);
                pos = keyEnd + 1;

                // Skip colon
                while (pos < body.length() && body.charAt(pos) != ':') pos++;
                pos++; // skip ':'
                while (pos < body.length() && Character.isWhitespace(body.charAt(pos))) pos++;

                if (!first) result.append(",");
                first = false;

                if (body.charAt(pos) == '"') {
                    // String value — SHA-256 hash it
                    int valStart = pos + 1;
                    int valEnd = body.indexOf('"', valStart);
                    String val = body.substring(valStart, valEnd);
                    pos = valEnd + 1;

                    try {
                        MessageDigest md = MessageDigest.getInstance("SHA-256");
                        byte[] hash = md.digest(val.getBytes(StandardCharsets.UTF_8));
                        result.append("\"").append(jsonEscape(key)).append("\":\"").append(hexEncode(hash)).append("\"");
                    } catch (Exception e) {
                        result.append("\"").append(jsonEscape(key)).append("\":\"error\"");
                    }
                } else if (body.charAt(pos) == '[') {
                    // Array value — reverse it
                    int depth = 0;
                    int arrStart = pos;
                    for (int i = pos; i < body.length(); i++) {
                        if (body.charAt(i) == '[') depth++;
                        else if (body.charAt(i) == ']') {
                            depth--;
                            if (depth == 0) { pos = i + 1; break; }
                        }
                    }
                    String arrStr = body.substring(arrStart + 1, pos - 1).trim();
                    // Split array elements (handles strings and numbers)
                    List<String> elements = new ArrayList<>();
                    int elemStart = 0;
                    int elemDepth = 0;
                    boolean inStr = false;
                    for (int i = 0; i < arrStr.length(); i++) {
                        char c = arrStr.charAt(i);
                        if (c == '"' && (i == 0 || arrStr.charAt(i - 1) != '\\')) inStr = !inStr;
                        if (!inStr) {
                            if (c == '[' || c == '{') elemDepth++;
                            else if (c == ']' || c == '}') elemDepth--;
                            else if (c == ',' && elemDepth == 0) {
                                elements.add(arrStr.substring(elemStart, i).trim());
                                elemStart = i + 1;
                            }
                        }
                    }
                    if (elemStart < arrStr.length()) {
                        String last = arrStr.substring(elemStart).trim();
                        if (!last.isEmpty()) elements.add(last);
                    }
                    Collections.reverse(elements);
                    result.append("\"").append(jsonEscape(key)).append("\":[");
                    for (int i = 0; i < elements.size(); i++) {
                        if (i > 0) result.append(",");
                        result.append(elements.get(i));
                    }
                    result.append("]");
                } else {
                    // Number or other — pass through
                    int valStart = pos;
                    while (pos < body.length() && body.charAt(pos) != ',' && body.charAt(pos) != '}') pos++;
                    String val = body.substring(valStart, pos).trim();
                    result.append("\"").append(jsonEscape(key)).append("\":").append(val);
                }

                // Skip comma
                while (pos < body.length() && Character.isWhitespace(body.charAt(pos))) pos++;
                if (pos < body.length() && body.charAt(pos) == ',') pos++;
            }

            result.append("}");
            sendAPI(ex, start, result.toString());
        }
    }

    // GET /api/aggregate?range=start,end — statistics over generated data points.
    static class APIAggregateHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            String rangeStr = params.getOrDefault("range", "");
            String[] parts = rangeStr.split(",", 2);
            if (parts.length != 2) {
                sendText(ex, 400, "{\"error\":\"range must be start,end\"}");
                return;
            }
            long rangeStart, rangeEnd;
            try {
                rangeStart = Long.parseLong(parts[0].trim());
                rangeEnd = Long.parseLong(parts[1].trim());
            } catch (NumberFormatException e) {
                sendText(ex, 400, "{\"error\":\"invalid range values\"}");
                return;
            }

            Random rng = new Random(rangeStart);
            int n = 10000;
            double[] values = new double[n];
            double sum = 0.0;

            // Category accumulators
            int[] catCount = new int[5];
            double[] catSum = new double[5];

            for (int i = 0; i < n; i++) {
                double v = rng.nextDouble() * (rangeEnd - rangeStart) + rangeStart;
                values[i] = v;
                sum += v;
                int catIdx = i % 5;
                catCount[catIdx]++;
                catSum[catIdx] += v;
            }

            Arrays.sort(values);

            StringBuilder json = new StringBuilder();
            json.append("{\"count\":").append(n);
            json.append(",\"mean\":").append(sum / n);
            json.append(",\"p50\":").append(values[n / 2]);
            json.append(",\"p95\":").append(values[(int) (n * 0.95)]);
            json.append(",\"max\":").append(values[n - 1]);
            json.append(",\"categories\":{");
            for (int c = 0; c < 5; c++) {
                if (c > 0) json.append(",");
                double mean = catCount[c] > 0 ? catSum[c] / catCount[c] : 0.0;
                json.append("\"").append(CAT_NAMES[c]).append("\":{");
                json.append("\"count\":").append(catCount[c]);
                json.append(",\"sum\":").append(catSum[c]);
                json.append(",\"mean\":").append(mean);
                json.append("}");
            }
            json.append("}}");

            sendAPI(ex, start, json.toString());
        }
    }

    // GET /api/search?q=term&limit=N — regex search over generated strings.
    static class APISearchHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            String q = params.getOrDefault("q", "");
            if (q.isEmpty()) {
                sendText(ex, 400, "{\"error\":\"q parameter required\"}");
                return;
            }
            int limit = 10;
            try { limit = Integer.parseInt(params.getOrDefault("limit", "10")); } catch (NumberFormatException ignored) {}
            if (limit < 1 || limit > 100) limit = 10;

            Pattern pattern = Pattern.compile(Pattern.quote(q), Pattern.CASE_INSENSITIVE);

            Random rng = new Random(42);
            List<int[]> matches = new ArrayList<>(); // [index, matchPos]
            List<String> matchTexts = new ArrayList<>();

            for (int i = 0; i < 1000; i++) {
                int wordCount = 3 + rng.nextInt(4);
                StringBuilder sb = new StringBuilder();
                for (int j = 0; j < wordCount; j++) {
                    if (j > 0) sb.append(" ");
                    sb.append(WORDS[rng.nextInt(WORDS.length)]);
                }
                String text = sb.toString();

                Matcher m = pattern.matcher(text);
                if (m.find()) {
                    matches.add(new int[]{i, m.start()});
                    matchTexts.add(text);
                }
            }

            // Compute scores and sort
            List<double[]> scored = new ArrayList<>(); // [origIdx, score]
            for (int i = 0; i < matches.size(); i++) {
                double score = 1.0 / (1.0 + matches.get(i)[1]);
                scored.add(new double[]{i, score});
            }
            scored.sort((a, b) -> Double.compare(b[1], a[1]));
            if (scored.size() > limit) scored = scored.subList(0, limit);

            StringBuilder json = new StringBuilder("[");
            for (int i = 0; i < scored.size(); i++) {
                if (i > 0) json.append(",");
                int idx = (int) scored.get(i)[0];
                json.append("{\"index\":").append(matches.get(idx)[0]);
                json.append(",\"text\":\"").append(jsonEscape(matchTexts.get(idx))).append("\"");
                json.append(",\"score\":").append(scored.get(i)[1]);
                json.append("}");
            }
            json.append("]");

            sendAPI(ex, start, json.toString());
        }
    }

    // POST /api/upload/process — hash and compress uploaded body.
    static class APIUploadProcessHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"POST".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);

            byte[] body;
            try (InputStream in = ex.getRequestBody()) {
                body = in.readAllBytes();
            }

            // CRC32
            CRC32 crc = new CRC32();
            crc.update(body);
            String crcHex = String.format("%08x", crc.getValue());

            // SHA-256
            String shaHex;
            try {
                MessageDigest md = MessageDigest.getInstance("SHA-256");
                shaHex = hexEncode(md.digest(body));
            } catch (Exception e) {
                shaHex = "error";
            }

            // Zlib compress (Deflater)
            Deflater deflater = new Deflater();
            deflater.setInput(body);
            deflater.finish();
            ByteArrayOutputStream compressed = new ByteArrayOutputStream();
            byte[] buf = new byte[8192];
            while (!deflater.finished()) {
                int n = deflater.deflate(buf);
                compressed.write(buf, 0, n);
            }
            deflater.end();

            String json = String.format(
                    "{\"original_size\":%d,\"compressed_size\":%d,\"crc32\":\"%s\",\"sha256\":\"%s\"}",
                    body.length, compressed.size(), crcHex, shaHex
            );

            sendAPI(ex, start, json);
        }
    }

    // GET /api/delayed?ms=N&work=light — sleep with optional CPU work.
    static class APIDelayedHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            int ms = 1;
            try { ms = Integer.parseInt(params.getOrDefault("ms", "1")); } catch (NumberFormatException ignored) {}
            if (ms < 1) ms = 1;
            if (ms > 100) ms = 100;

            String work = params.getOrDefault("work", "light");

            try { Thread.sleep(ms); } catch (InterruptedException ignored) {}

            double actualMs = (System.nanoTime() - start) / 1_000_000.0;

            StringBuilder json = new StringBuilder();
            json.append("{\"requested_ms\":").append(ms);
            json.append(",\"actual_ms\":").append(String.format("%.1f", actualMs));
            json.append(",\"work\":\"").append(jsonEscape(work)).append("\"");

            if ("heavy".equals(work)) {
                double x = 0.0;
                for (int i = 0; i < 100000; i++) {
                    x += Math.sqrt(i);
                }
                json.append(",\"compute\":").append(x);
            }

            json.append("}");

            sendAPI(ex, start, json.toString());
        }
    }

    // GET /api/validate?seed=42 — checksums for all endpoints at given seed.
    static class APIValidateHandler implements HttpHandler {
        @Override
        public void handle(HttpExchange ex) throws IOException {
            if (!"GET".equals(ex.getRequestMethod())) {
                sendText(ex, 405, "{\"error\":\"method not allowed\"}");
                return;
            }
            long start = setAPIHeaders(ex);
            Map<String, String> params = parseQuery(ex.getRequestURI().getRawQuery());

            long seed = 42;
            try { seed = Long.parseLong(params.getOrDefault("seed", "42")); } catch (NumberFormatException ignored) {}
            if (seed == 0) seed = 42;

            try {
                MessageDigest md = MessageDigest.getInstance("SHA-256");

                // Users checksum
                List<Map<String, Object>> users = generateUsers(seed);
                String usersJson = usersToJson(users);
                byte[] usersHash = md.digest(usersJson.getBytes(StandardCharsets.UTF_8));
                String usersHex = hexEncode(Arrays.copyOf(usersHash, 16));

                // Aggregate checksum
                md.reset();
                Random rng = new Random(seed);
                double sum = 0.0;
                for (int i = 0; i < 10000; i++) {
                    sum += rng.nextDouble() * 100.0;
                }
                byte[] aggHash = md.digest(String.format("%.6f", sum).getBytes(StandardCharsets.UTF_8));
                String aggHex = hexEncode(Arrays.copyOf(aggHash, 16));

                // Search checksum (seed=42 corpus, q="network")
                md.reset();
                Random rng2 = new Random(42);
                StringBuilder corpus = new StringBuilder();
                for (int i = 0; i < 1000; i++) {
                    int wordCount = 3 + rng2.nextInt(4);
                    for (int j = 0; j < wordCount; j++) {
                        if (j > 0) corpus.append(" ");
                        corpus.append(WORDS[rng2.nextInt(WORDS.length)]);
                    }
                    corpus.append("\n");
                }
                byte[] searchHash = md.digest(corpus.toString().getBytes(StandardCharsets.UTF_8));
                String searchHex = hexEncode(Arrays.copyOf(searchHash, 16));

                String json = String.format(
                        "{\"seed\":\"%d\",\"users\":\"%s\",\"aggregate\":\"%s\",\"search\":\"%s\"}",
                        seed, usersHex, aggHex, searchHex
                );

                sendAPI(ex, start, json);
            } catch (Exception e) {
                sendText(ex, 500, "{\"error\":\"internal error\"}");
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────

    private static void sendText(HttpExchange ex, int code, String body) throws IOException {
        byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
        ex.getResponseHeaders().set("Content-Type", "application/json");
        ex.sendResponseHeaders(code, bytes.length);
        try (OutputStream out = ex.getResponseBody()) {
            out.write(bytes);
        }
    }

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
