import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpHandler;
import com.sun.net.httpserver.HttpsConfigurator;
import com.sun.net.httpserver.HttpsParameters;
import com.sun.net.httpserver.HttpsServer;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyFactory;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.spec.PKCS8EncodedKeySpec;
import java.time.Instant;
import java.util.Base64;
import java.util.concurrent.Executors;

/**
 * AletheBench Java reference API.
 *
 * Single-file HTTPS server using JDK built-in com.sun.net.httpserver with
 * Virtual Threads (Java 21+). No external dependencies.
 *
 * Endpoints:
 *   GET  /health           -> {"status":"ok","language":"java","runtime":"..."}
 *   GET  /download/{size}  -> stream `size` bytes of zeros
 *   POST /upload           -> read body, return {"bytes_received": N}
 */
public class Server {

    private static final int PORT = 8443;
    private static final String CERT_DIR = System.getenv().getOrDefault("CERT_DIR", "/opt/bench");

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
