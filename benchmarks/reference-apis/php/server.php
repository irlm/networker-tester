<?php
/**
 * Networker Bench PHP reference API — Swoole async HTTP server.
 *
 * Conforms to benchmarks/shared/API-SPEC.md (frozen contract v1, family C).
 * Worker policy (§3): Swoole worker_num = BENCH_WORKERS (default = CPUs).
 */

declare(strict_types=1);

// ── Structured logging ────────────────────────────────────────────────────

$LOG_LEVEL = strtolower(getenv('LOG_LEVEL') ?: 'info');
$LOG_LEVELS = ['error' => 0, 'warn' => 1, 'info' => 2, 'debug' => 3];
$CURRENT_LEVEL = $LOG_LEVELS[$LOG_LEVEL] ?? 2;

function bench_log(string $level, string $msg): void
{
    global $CURRENT_LEVEL, $LOG_LEVELS;
    if (($LOG_LEVELS[$level] ?? 0) <= $CURRENT_LEVEL) {
        $ts = date('Y-m-d H:i:s');
        fwrite(STDERR, "[$ts] " . strtoupper($level) . " $msg\n");
    }
}

// ── Shared benchmark dataset (spec §2) — load failure is FATAL ────────────

function bench_fatal(string $msg): void
{
    fwrite(STDERR, "FATAL: $msg\n");
    exit(1);
}

function load_bench_data(): array
{
    $envPath = getenv('BENCH_DATA_PATH');
    if ($envPath !== false && $envPath !== '') {
        // An explicitly configured path must exist and parse (spec §2).
        $raw = @file_get_contents($envPath);
        if ($raw === false) {
            bench_fatal("BENCH_DATA_PATH=$envPath could not be read");
        }
        $data = json_decode($raw, true);
        if (!is_array($data)) {
            bench_fatal("BENCH_DATA_PATH=$envPath is not valid JSON: " . json_last_error_msg());
        }
        $source = $envPath;
    } else {
        $data = null;
        $source = '';
        foreach (['/opt/bench/bench-data.json', __DIR__ . '/../shared/bench-data.json'] as $p) {
            if (!file_exists($p)) {
                continue;
            }
            $raw = @file_get_contents($p);
            $data = $raw === false ? null : json_decode($raw, true);
            if (!is_array($data)) {
                bench_fatal("bench-data.json exists at $p but could not be loaded: " . json_last_error_msg());
            }
            $source = $p;
            break;
        }
        if ($data === null) {
            bench_fatal('bench-data.json not found (set BENCH_DATA_PATH or deploy ' .
                '/opt/bench/bench-data.json); reference implementations have no ' .
                'PRNG fallback (spec §2)');
        }
    }

    // Verify the §2 schema counts.
    $checks = [
        '_version == 2'                => ($data['_version'] ?? 0) === 2,
        'users == 100'                 => count($data['users'] ?? []) === 100,
        'search_corpus == 1000'        => count($data['search_corpus'] ?? []) === 1000,
        'timeseries == 10000'          => count($data['timeseries'] ?? []) === 10000,
        'transform_inputs == 10'       => count($data['transform_inputs'] ?? []) === 10,
        'expected_checksums == 4 keys' => count($data['expected_checksums'] ?? []) === 4,
    ];
    foreach ($checks as $check => $ok) {
        if (!$ok) {
            bench_fatal("bench-data.json at $source: schema check failed ($check)");
        }
    }

    bench_log('info', "Loaded bench-data.json from $source (_version 2, 100 users, 1000 corpus, 10000 timeseries)");
    return $data;
}

$BENCH_DATA = load_bench_data();
$USERS = $BENCH_DATA['users'];
$CORPUS = $BENCH_DATA['search_corpus'];
// Aggregate reads only the value field, in dataset order (spec §5.6).
$TS_VALUES = array_map(static fn ($p) => (float) $p['value'], $BENCH_DATA['timeseries']);
$CHECKSUMS = $BENCH_DATA['expected_checksums'];

// /health body is a byte-constant precomputed at startup (spec §5.1).
$HEALTH_BODY = json_encode(
    ['status' => 'ok', 'runtime' => 'php', 'version' => PHP_VERSION],
    JSON_UNESCAPED_SLASHES
);

const CHUNK_SIZE = 8192;               // spec §5.2: pinned chunk size
const DOWNLOAD_CAP = 2147483648;       // spec §5.2: 2 GiB cap
// Swoole's write() forces chunked transfer (no Content-Length header), so
// bodies up to this threshold are sent via end() with an exact
// Content-Length; larger bodies stream in 8 KiB chunks (chunked encoding).
const DOWNLOAD_BUFFER_THRESHOLD = 8388608; // 8 MiB

$USER_SORT_FIELDS = ['id', 'name', 'email', 'score', 'created_at'];
$USER_STRING_FIELDS = ['name' => true, 'email' => true, 'created_at' => true];

// ── Helpers ────────────────────────────────────────────────────────────────

/** Spec §5.6: round half away from zero to 2 decimals (float64 semantics). */
function r2(float $x): float
{
    return floor($x * 100 + 0.5) / 100;
}

function api_headers(Swoole\HTTP\Response $response, float $duration_ms): void
{
    $response->header('Content-Type', 'application/json');
    $response->header('Server-Timing', sprintf('app;dur=%.1f', $duration_ms));
    $response->header('Cache-Control', 'no-store, no-cache, must-revalidate');
    $response->header('Timing-Allow-Origin', '*');
    $response->header('Access-Control-Allow-Origin', '*');
}

function api_json(Swoole\HTTP\Response $response, array $body, float $duration_ms, int $status = 200): void
{
    $response->status($status);
    api_headers($response, $duration_ms);
    // JSON_PRESERVE_ZERO_FRACTION: the frozen dataset's aggregate contains an
    // exact 39.0 -- without the flag PHP emits `39`, the Python §7
    // canonicalizer reads int != float, and the pinned checksum diverges (the
    // same int-vs-float hazard the C#/Go/Java ports hit; caught by the first
    // HARD-tier validation run).
    $response->end(json_encode($body, JSON_UNESCAPED_SLASHES | JSON_PRESERVE_ZERO_FRACTION));
}

function int_param(array $params, string $key, int $default): int
{
    $raw = $params[$key] ?? null;
    if ($raw === null || !preg_match('/\A-?\d+\z/', (string) $raw)) {
        return $default;
    }
    return (int) $raw;
}

// ── Server ─────────────────────────────────────────────────────────────────

$certDir = getenv('BENCH_CERT_DIR') ?: '/opt/bench';
$port = (int) (getenv('BENCH_PORT') ?: 8443);
$workers = (int) (getenv('BENCH_WORKERS') ?: swoole_cpu_num());
if ($workers < 1) {
    $workers = swoole_cpu_num();
}

// Listener type is chosen at startup from cert presence (audit F8): certs
// absent → plain HTTP on the same port (application mode behind a
// TLS-terminating reverse proxy), mirroring the Go/Node/Java pattern.
$hasTls = is_file("$certDir/cert.pem") && is_file("$certDir/key.pem");

$server = new Swoole\HTTP\Server(
    '0.0.0.0',
    $port,
    SWOOLE_PROCESS,
    $hasTls ? (SWOOLE_SOCK_TCP | SWOOLE_SSL) : SWOOLE_SOCK_TCP
);

$settings = [
    // Worker policy (spec §3): BENCH_WORKERS, default = logical CPU count.
    'worker_num'         => $workers,
    // Request handlers run in coroutines so /api/delayed can use a
    // non-blocking coroutine sleep (spec §5.9, audit F7).
    'enable_coroutine'   => true,
    // Swoole buffers request bodies; allow benchmark-sized uploads.
    'package_max_length' => 64 * 1024 * 1024,
];
if ($hasTls) {
    $settings['ssl_cert_file'] = "$certDir/cert.pem";
    $settings['ssl_key_file']  = "$certDir/key.pem";
} else {
    bench_log('info', "no TLS certs in $certDir - serving plain HTTP on port $port (application mode)");
}
$server->set($settings);

$BENCH_API_TOKEN = getenv('BENCH_API_TOKEN') ?: '';

// ── Request handler ────────────────────────────────────────────────────────

$server->on('request', function (
    Swoole\HTTP\Request $request,
    Swoole\HTTP\Response $response
) use ($USERS, $CORPUS, $TS_VALUES, $CHECKSUMS, $HEALTH_BODY, $BENCH_API_TOKEN, $USER_SORT_FIELDS, $USER_STRING_FIELDS) {
    $path   = $request->server['request_uri'] ?? '/';
    $method = $request->server['request_method'] ?? 'GET';
    // HEAD is served by the GET handler (Swoole suppresses the body); the
    // validator's header checks use HEAD, mirroring axum auto-HEAD behaviour.
    if ($method === 'HEAD') {
        $method = 'GET';
    }
    $params = $request->get ?? [];

    // Bearer token authentication — every route except /health (spec §1).
    if ($BENCH_API_TOKEN !== '' && $path !== '/health') {
        $auth = $request->header['authorization'] ?? '';
        if ($auth !== 'Bearer ' . $BENCH_API_TOKEN) {
            $response->status(401);
            $response->header('Content-Type', 'application/json');
            $response->end('{"error":"unauthorized"}');
            return;
        }
    }

    // GET /health — constant-work byte-constant body (spec §5.1).
    if ($method === 'GET' && $path === '/health') {
        $response->header('Content-Type', 'application/json');
        $response->end($HEALTH_BODY);
        return;
    }

    // GET /download/{size} — 0x42 in 8 KiB chunks (spec §5.2).
    if ($method === 'GET' && str_starts_with($path, '/download/')) {
        $t0 = hrtime(true);
        $sizeStr = substr($path, strlen('/download/'));
        if (!preg_match('/\A\d+\z/', $sizeStr)) { // non-integer → 400
            $response->status(400);
            $response->header('Content-Type', 'application/json');
            $response->end('{"error":"invalid size"}');
            return;
        }
        $size = min((int) $sizeStr, DOWNLOAD_CAP); // clamp above cap; 0 is valid

        $response->header('Content-Type', 'application/octet-stream');
        $response->header('X-Download-Bytes', (string) $size);
        $response->header('Server-Timing', sprintf('proc;dur=%.1f', (hrtime(true) - $t0) / 1e6));

        if ($size <= DOWNLOAD_BUFFER_THRESHOLD) {
            $response->header('Content-Length', (string) $size);
            $response->end(str_repeat("\x42", $size));
            return;
        }
        $chunk = str_repeat("\x42", CHUNK_SIZE);
        $remaining = $size;
        while ($remaining > 0) {
            $toSend = min($remaining, CHUNK_SIZE);
            $response->write($toSend === CHUNK_SIZE ? $chunk : substr($chunk, 0, $toSend));
            $remaining -= $toSend;
        }
        $response->end();
        return;
    }

    // POST /upload (spec §5.3).
    if ($method === 'POST' && $path === '/upload') {
        $t0 = hrtime(true);
        $body = $request->rawContent();
        $received = $body !== false ? strlen($body) : 0;
        $response->header('Content-Type', 'application/json');
        $response->header('X-Networker-Received-Bytes', (string) $received);
        $response->header('Server-Timing', sprintf('recv;dur=%.1f', (hrtime(true) - $t0) / 1e6));
        $requestId = $request->header['x-networker-request-id'] ?? null;
        if ($requestId !== null) {
            $response->header('X-Networker-Request-Id', $requestId);
        }
        $response->end(json_encode(['received_bytes' => $received]));
        return;
    }

    // ── JSON API endpoints (family C, spec §5.4-§5.10) ────────────────────

    // GET /api/users?page=N&sort=<field>&order=<asc|desc>
    if ($method === 'GET' && $path === '/api/users') {
        $t0 = hrtime(true);
        $page      = max(1, int_param($params, 'page', 1));
        $sortField = $params['sort'] ?? 'id';
        if (!in_array($sortField, $USER_SORT_FIELDS, true)) {
            $sortField = 'id';
        }
        $desc = ($params['order'] ?? 'asc') === 'desc';

        $window = array_slice($USERS, ($page - 1) * 100, 100);
        $isString = isset($USER_STRING_FIELDS[$sortField]);
        // usort is stable in PHP >= 8.0; desc reverses the comparator so
        // ties stay in dataset order (family C semantics).
        usort($window, function ($a, $b) use ($sortField, $desc, $isString) {
            $cmp = $isString
                ? strcmp((string) $a[$sortField], (string) $b[$sortField]) // bytewise
                : ($a[$sortField] <=> $b[$sortField]);
            return $desc ? -$cmp : $cmp;
        });

        // Bare JSON array of the first 20 users of the sorted window.
        api_json($response, array_slice($window, 0, 20), (hrtime(true) - $t0) / 1e6);
        return;
    }

    // POST /api/transform
    if ($method === 'POST' && $path === '/api/transform') {
        $t0  = hrtime(true);
        $raw = $request->rawContent();
        $body = json_decode($raw !== false ? $raw : '', true);

        if (!is_array($body)) { // syntactically invalid JSON (or non-object) → 400
            api_json($response, ['error' => 'invalid JSON'], (hrtime(true) - $t0) / 1e6, 400);
            return;
        }

        $fields = $body['fields'] ?? [];
        $values = $body['values'] ?? [];
        api_json($response, [
            'seed'            => $body['seed'] ?? 0,
            'hashed_fields'   => array_map(static fn ($f) => hash('sha256', (string) $f), is_array($fields) ? $fields : []),
            'reversed_values' => array_reverse(is_array($values) ? $values : []),
        ], (hrtime(true) - $t0) / 1e6);
        return;
    }

    // GET /api/aggregate — `range` accepted and ignored (spec §5.6).
    if ($method === 'GET' && $path === '/api/aggregate') {
        $t0 = hrtime(true);
        $values = $TS_VALUES;
        sort($values);
        $n = count($values);
        $total = 0.0;
        foreach ($values as $v) { // sequential sum over SORTED values
            $total += $v;
        }
        $chunk = intdiv($n, 5);
        $categories = [];
        for ($i = 0; $i < 5; $i++) {
            $part = array_slice($values, $i * $chunk, $chunk);
            $s = 0.0;
            foreach ($part as $v) {
                $s += $v;
            }
            $categories[] = [
                'category' => 'q' . ($i + 1),
                'count'    => $chunk,
                'mean'     => r2($s / $chunk),
                'min'      => r2($part[0]),
                'max'      => r2($part[$chunk - 1]),
            ];
        }
        api_json($response, [
            'total_points' => $n,
            'mean'         => r2($total / $n),
            'p50'          => r2($values[(int) ($n * 0.50)]),
            'p95'          => r2($values[(int) ($n * 0.95)]),
            'max'          => r2($values[$n - 1]),
            'categories'   => $categories,
        ], (hrtime(true) - $t0) / 1e6);
        return;
    }

    // GET /api/search?q=<term>&limit=N
    if ($method === 'GET' && $path === '/api/search') {
        $t0    = hrtime(true);
        $query = (string) ($params['q'] ?? 'test');
        $limit = min(int_param($params, 'limit', 20), 100);

        // Case-sensitive regex; literal substring fallback on invalid pattern.
        $pattern = '/' . str_replace('/', '\/', $query) . '/';
        $regexOk = @preg_match($pattern, '') !== false;

        $matches = [];
        foreach ($CORPUS as $item) {
            if ($regexOk) {
                if (preg_match($pattern, $item, $m, PREG_OFFSET_CAPTURE)) {
                    $matches[] = [$m[0][1], $item]; // byte offset of first match
                }
            } else {
                $pos = strpos($item, $query);
                if ($pos !== false) {
                    $matches[] = [$pos, $item];
                }
            }
        }
        usort($matches, static function ($a, $b) {
            $c = $a[0] <=> $b[0];               // position asc
            return $c !== 0 ? $c : strcmp($a[1], $b[1]); // item asc bytewise
        });

        $results = [];
        foreach (array_slice($matches, 0, max($limit, 0)) as $i => [$pos, $item]) {
            $results[] = ['rank' => $i + 1, 'item' => $item, 'match_position' => $pos];
        }
        api_json($response, [
            'query'         => $query,
            'total_matches' => count($matches), // counted BEFORE truncation
            'returned'      => count($results),
            'results'       => $results,
        ], (hrtime(true) - $t0) / 1e6);
        return;
    }

    // POST /api/upload/process (spec §5.8).
    if ($method === 'POST' && $path === '/api/upload/process') {
        $t0   = hrtime(true);
        $body = $request->rawContent();
        $body = $body !== false ? $body : '';

        api_json($response, [
            'original_size'   => strlen($body),
            // zlib (RFC 1950) at level 6 — gzcompress emits a zlib stream.
            'compressed_size' => strlen(gzcompress($body, 6)),
            'crc32'           => sprintf('%08x', crc32($body) & 0xFFFFFFFF),
            'sha256'          => hash('sha256', $body),
        ], (hrtime(true) - $t0) / 1e6);
        return;
    }

    // GET /api/delayed?ms=N — coroutine sleep, ms clamped to [1,100] (spec §5.9).
    if ($method === 'GET' && $path === '/api/delayed') {
        $t0 = hrtime(true);
        $ms = max(1, min(100, int_param($params, 'ms', 10)));
        // `work` is reserved: accepted and ignored.

        // Coroutine sleep yields the worker instead of blocking it (audit F7).
        \Swoole\Coroutine::sleep($ms / 1000.0);

        $actualMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'requested_ms' => $ms,
            'actual_ms'    => round($actualMs, 2),
        ], $actualMs);
        return;
    }

    // GET /api/validate?seed=N — echo dataset checksums (spec §5.10).
    if ($method === 'GET' && $path === '/api/validate') {
        $t0   = hrtime(true);
        $seed = int_param($params, 'seed', 42);
        api_json($response, [
            'seed'      => $seed,
            'checksums' => $CHECKSUMS,
        ], (hrtime(true) - $t0) / 1e6);
        return;
    }

    // 404
    $response->status(404);
    $response->header('Content-Type', 'application/json');
    $response->end('{"error":"not found"}');
});

bench_log('info', 'PHP Swoole server starting on ' . ($hasTls ? 'https' : 'http') . "://0.0.0.0:$port ($workers workers)");
$server->start();
