<?php
/**
 * AletheBench PHP reference API — Swoole async HTTP server.
 */

declare(strict_types=1);

$certDir = getenv('BENCH_CERT_DIR') ?: '/opt/bench';

$server = new Swoole\HTTP\Server(
    '0.0.0.0',
    8443,
    SWOOLE_PROCESS,
    SWOOLE_SOCK_TCP | SWOOLE_SSL
);

$server->set([
    'ssl_cert_file' => "$certDir/cert.pem",
    'ssl_key_file'  => "$certDir/key.pem",
    'worker_num'    => 4,
]);

const CHUNK_SIZE = 8192;
$chunk = str_repeat("\x42", CHUNK_SIZE);

$server->on('request', function (
    Swoole\HTTP\Request $request,
    Swoole\HTTP\Response $response
) use ($chunk) {
    $path   = $request->server['request_uri'] ?? '/';
    $method = $request->server['request_method'] ?? 'GET';

    // GET /health
    if ($method === 'GET' && $path === '/health') {
        $response->header('Content-Type', 'application/json');
        $response->end(json_encode([
            'status'  => 'ok',
            'runtime' => 'php',
            'version' => PHP_VERSION,
        ]));
        return;
    }

    // GET /download/{size}
    if ($method === 'GET' && preg_match('#^/download/(\d+)$#', $path, $matches)) {
        $size = (int) $matches[1];
        if ($size <= 0) {
            $response->status(400);
            $response->header('Content-Type', 'application/json');
            $response->end('{"error":"invalid size"}');
            return;
        }

        $response->header('Content-Type', 'application/octet-stream');
        $response->header('Content-Length', (string) $size);

        $remaining = $size;
        while ($remaining > 0) {
            $toSend = min($remaining, CHUNK_SIZE);
            $response->write(substr($chunk, 0, $toSend));
            $remaining -= $toSend;
        }
        $response->end();
        return;
    }

    // POST /upload
    if ($method === 'POST' && $path === '/upload') {
        $body = $request->rawContent();
        $bytesReceived = $body !== false ? strlen($body) : 0;
        $response->header('Content-Type', 'application/json');
        $response->end(json_encode(['bytes_received' => $bytesReceived]));
        return;
    }

    // 404
    $response->status(404);
    $response->header('Content-Type', 'application/json');
    $response->end('{"error":"not found"}');
});

echo "PHP Swoole server starting on https://0.0.0.0:8443\n";
$server->start();
