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

// ── Shared data for API endpoints ──────────────────────────────────────────

const FIRST_NAMES = [
    'Alice', 'Bob', 'Carol', 'Dave', 'Eve', 'Frank', 'Grace', 'Heidi',
    'Ivan', 'Judy', 'Karl', 'Laura', 'Mallory', 'Nina', 'Oscar', 'Peggy',
    'Quentin', 'Ruth', 'Steve', 'Trent', 'Ursula', 'Victor', 'Wendy',
    'Xander', 'Yvonne', 'Zack',
];

const LAST_NAMES = [
    'Smith', 'Johnson', 'Williams', 'Brown', 'Jones', 'Garcia', 'Miller',
    'Davis', 'Rodriguez', 'Martinez', 'Hernandez', 'Lopez', 'Gonzalez',
    'Wilson', 'Anderson', 'Thomas', 'Taylor', 'Moore', 'Jackson', 'Martin',
];

const DEPARTMENTS = [
    'Engineering', 'Marketing', 'Sales', 'Finance', 'HR',
    'Operations', 'Legal', 'Support', 'Design', 'Product',
];

const SEARCH_WORDS = [
    'network', 'latency', 'throughput', 'bandwidth', 'packet',
    'routing', 'firewall', 'proxy', 'endpoint', 'server',
    'client', 'protocol', 'socket', 'buffer', 'stream',
    'timeout', 'retry', 'cache', 'queue', 'load',
];

// ── Helpers ────────────────────────────────────────────────────────────────

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
    $response->end(json_encode($body, JSON_UNESCAPED_SLASHES));
}

function generate_users(int $seed, int $count = 100): array
{
    mt_srand($seed);
    $users = [];
    for ($i = 0; $i < $count; $i++) {
        $users[] = [
            'id'         => $i + 1,
            'name'       => FIRST_NAMES[mt_rand(0, count(FIRST_NAMES) - 1)] . ' '
                          . LAST_NAMES[mt_rand(0, count(LAST_NAMES) - 1)],
            'email'      => 'user' . ($i + 1) . '@example.com',
            'age'        => mt_rand(22, 65),
            'department' => DEPARTMENTS[mt_rand(0, count(DEPARTMENTS) - 1)],
            'score'      => round(mt_rand(0, 10000) / 100, 2),
        ];
    }
    return $users;
}

function gauss_random(float $mean, float $stddev): float
{
    $u1 = mt_rand(1, PHP_INT_MAX) / PHP_INT_MAX;
    $u2 = mt_rand(1, PHP_INT_MAX) / PHP_INT_MAX;
    $z  = sqrt(-2.0 * log($u1)) * cos(2.0 * M_PI * $u2);
    return $mean + $stddev * $z;
}

// ── Request handler ────────────────────────────────────────────────────────

$server->on('request', function (
    Swoole\HTTP\Request $request,
    Swoole\HTTP\Response $response
) use ($chunk) {
    $path   = $request->server['request_uri'] ?? '/';
    $method = $request->server['request_method'] ?? 'GET';
    $params = $request->get ?? [];

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

    // ── JSON API endpoints ─────────────────────────────────────────────────

    // GET /api/users?page=N&sort=field&order=asc|desc
    if ($method === 'GET' && $path === '/api/users') {
        $t0 = hrtime(true);
        $page       = (int) ($params['page'] ?? 1);
        $sortField  = $params['sort'] ?? 'id';
        $order      = $params['order'] ?? 'asc';

        $users = generate_users($page);

        $validFields = ['id', 'name', 'email', 'age', 'department', 'score'];
        if (in_array($sortField, $validFields, true)) {
            usort($users, function ($a, $b) use ($sortField, $order) {
                $cmp = $a[$sortField] <=> $b[$sortField];
                return $order === 'desc' ? -$cmp : $cmp;
            });
        }

        $pageSize  = 20;
        $start     = ($page - 1) * $pageSize;
        $pageUsers = array_slice($users, $start, $pageSize);

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'page'      => $page,
            'page_size' => $pageSize,
            'total'     => count($users),
            'sort'      => $sortField,
            'order'     => $order,
            'users'     => $pageUsers,
        ], $durationMs);
        return;
    }

    // POST /api/transform
    if ($method === 'POST' && $path === '/api/transform') {
        $t0  = hrtime(true);
        $raw = $request->rawContent();
        $body = json_decode($raw ?: '', true);

        if (!is_array($body)) {
            $durationMs = (hrtime(true) - $t0) / 1e6;
            api_json($response, ['error' => 'invalid JSON'], $durationMs, 400);
            return;
        }

        $transformed = [];
        foreach ($body as $key => $value) {
            if (is_string($value)) {
                $hashed = hash('sha256', $value);
                $transformed[$key] = [
                    'original_reversed' => strrev($value),
                    'sha256'            => $hashed,
                ];
            } else {
                $transformed[$key] = $value;
            }
        }

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'original_fields' => count($body),
            'transformed'     => $transformed,
        ], $durationMs);
        return;
    }

    // GET /api/aggregate?range=start,end
    if ($method === 'GET' && $path === '/api/aggregate') {
        $t0 = hrtime(true);
        $rangeParam = $params['range'] ?? '0,1000';
        $parts      = explode(',', $rangeParam);
        $rangeStart = (int) ($parts[0] ?? 0);
        $rangeEnd   = (int) ($parts[1] ?? 1000);

        mt_srand($rangeStart);
        $count  = 10000;
        $values = [];
        for ($i = 0; $i < $count; $i++) {
            $values[] = gauss_random(50, 15);
        }

        $categories = ['alpha', 'beta', 'gamma', 'delta', 'epsilon'];
        mt_srand($rangeStart + 1);
        $assignments = [];
        for ($i = 0; $i < $count; $i++) {
            $assignments[] = $categories[mt_rand(0, count($categories) - 1)];
        }

        $sortedVals = $values;
        sort($sortedVals);
        $mean   = array_sum($values) / $count;
        $p50    = $sortedVals[(int) ($count / 2)];
        $p95    = $sortedVals[(int) ($count * 0.95)];
        $maxVal = $sortedVals[$count - 1];

        $groups = [];
        foreach ($categories as $cat) {
            $catVals = [];
            for ($i = 0; $i < $count; $i++) {
                if ($assignments[$i] === $cat) {
                    $catVals[] = $values[$i];
                }
            }
            if (!empty($catVals)) {
                sort($catVals);
                $groups[$cat] = [
                    'count' => count($catVals),
                    'mean'  => round(array_sum($catVals) / count($catVals), 4),
                    'p50'   => round($catVals[(int) (count($catVals) / 2)], 4),
                    'max'   => round($catVals[count($catVals) - 1], 4),
                ];
            }
        }

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'range'        => ['start' => $rangeStart, 'end' => $rangeEnd],
            'total_points' => $count,
            'stats'        => [
                'mean' => round($mean, 4),
                'p50'  => round($p50, 4),
                'p95'  => round($p95, 4),
                'max'  => round($maxVal, 4),
            ],
            'groups' => $groups,
        ], $durationMs);
        return;
    }

    // GET /api/search?q=term&limit=N
    if ($method === 'GET' && $path === '/api/search') {
        $t0    = hrtime(true);
        $query = $params['q'] ?? 'test';
        $limit = (int) ($params['limit'] ?? 10);

        mt_srand(42);
        $corpus = [];
        for ($i = 0; $i < 1000; $i++) {
            $wordCount = mt_rand(3, 8);
            $words     = [];
            for ($j = 0; $j < $wordCount; $j++) {
                $words[] = SEARCH_WORDS[mt_rand(0, count(SEARCH_WORDS) - 1)];
            }
            $corpus[] = ['id' => $i + 1, 'text' => implode(' ', $words)];
        }

        $pattern = '@' . str_replace('@', '\\@', $query) . '@i';
        if (@preg_match($pattern, '') === false) {
            $durationMs = (hrtime(true) - $t0) / 1e6;
            api_json($response, ['error' => 'invalid regex'], $durationMs, 400);
            return;
        }

        $results = [];
        foreach ($corpus as $item) {
            if (preg_match($pattern, $item['text'], $m, PREG_OFFSET_CAPTURE)) {
                $pos     = $m[0][1];
                $score   = 1.0 / (1 + $pos);
                $results[] = [
                    'id'    => $item['id'],
                    'text'  => $item['text'],
                    'score' => round($score, 4),
                ];
            }
        }

        usort($results, fn($a, $b) => $b['score'] <=> $a['score']);
        $results = array_slice($results, 0, $limit);

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'query'         => $query,
            'total_matches' => count($results),
            'limit'         => $limit,
            'results'       => $results,
        ], $durationMs);
        return;
    }

    // POST /api/upload/process
    if ($method === 'POST' && $path === '/api/upload/process') {
        $t0   = hrtime(true);
        $body = $request->rawContent();
        $body = $body !== false ? $body : '';

        $crc        = crc32($body) & 0xFFFFFFFF;
        $sha        = hash('sha256', $body);
        $compressed = gzcompress($body);

        $origSize = strlen($body);
        $compSize = strlen($compressed);

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'original_size'     => $origSize,
            'compressed_size'   => $compSize,
            'compression_ratio' => round($compSize / max($origSize, 1), 4),
            'crc32'             => sprintf('%08x', $crc),
            'sha256'            => $sha,
        ], $durationMs);
        return;
    }

    // GET /api/delayed?ms=N&work=light
    if ($method === 'GET' && $path === '/api/delayed') {
        $t0   = hrtime(true);
        $ms   = max(1, min(100, (int) ($params['ms'] ?? 100)));
        $work = $params['work'] ?? 'none';

        usleep($ms * 1000);

        if ($work === 'light') {
            hash('sha256', str_repeat('benchmark', 100));
        }

        $actualMs   = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'requested_ms' => $ms,
            'actual_ms'    => round($actualMs, 2),
            'work'         => $work,
        ], $actualMs);
        return;
    }

    // GET /api/validate?seed=42
    if ($method === 'GET' && $path === '/api/validate') {
        $t0   = hrtime(true);
        $seed = (int) ($params['seed'] ?? 42);

        // Users checksum (page=1)
        $users = generate_users(1);
        // Sort each user's keys for deterministic JSON
        $usersForHash = array_map(function ($u) {
            ksort($u);
            return $u;
        }, $users);
        $usersHash = hash('sha256', json_encode($usersForHash, JSON_UNESCAPED_SLASHES));

        // Aggregate checksum (start=0)
        mt_srand(0);
        $values = [];
        for ($i = 0; $i < 10000; $i++) {
            $values[] = round(gauss_random(50, 15), 4);
        }
        sort($values);
        $aggHash = hash('sha256', json_encode($values));

        // Search checksum
        mt_srand(42);
        $corpus = [];
        for ($i = 0; $i < 1000; $i++) {
            $wordCount = mt_rand(3, 8);
            $words     = [];
            for ($j = 0; $j < $wordCount; $j++) {
                $words[] = SEARCH_WORDS[mt_rand(0, count(SEARCH_WORDS) - 1)];
            }
            $corpus[] = implode(' ', $words);
        }
        $searchHash = hash('sha256', json_encode($corpus));

        $durationMs = (hrtime(true) - $t0) / 1e6;
        api_json($response, [
            'seed'      => $seed,
            'checksums' => [
                'users_page1'      => substr($usersHash, 0, 16),
                'aggregate_start0' => substr($aggHash, 0, 16),
                'search_corpus'    => substr($searchHash, 0, 16),
            ],
        ], $durationMs);
        return;
    }

    // 404
    $response->status(404);
    $response->header('Content-Type', 'application/json');
    $response->end('{"error":"not found"}');
});

echo "PHP Swoole server starting on https://0.0.0.0:8443\n";
$server->start();
