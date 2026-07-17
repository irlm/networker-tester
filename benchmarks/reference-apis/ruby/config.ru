# Networker Bench Ruby reference API — direct Rack app on Puma.
#
# Conforms to benchmarks/shared/API-SPEC.md (frozen contract v1, family C).
# Worker policy (§3): puma cluster workers = BENCH_WORKERS (see puma.rb),
# 5:5 threads per worker.

require 'digest/sha2'
require 'json'
require 'logger'
require 'uri'
require 'zlib'

LOGGER = Logger.new($stderr)
LOGGER.level = ENV.fetch('LOG_LEVEL', 'INFO').upcase == 'DEBUG' ? Logger::DEBUG : Logger::INFO

BENCH_API_TOKEN = ENV['BENCH_API_TOKEN'] || ''

CHUNK_SIZE = 8192 # spec §5.2: pinned chunk size
CHUNK = ("\x42" * CHUNK_SIZE).b.freeze # spec §5.2: pinned fill byte 0x42
DOWNLOAD_CAP = 2_147_483_648 # spec §5.2: 2 GiB cap

# ── Shared benchmark dataset (spec §2) — load failure is FATAL ──────────────

def load_bench_data!
  env_path = ENV['BENCH_DATA_PATH']
  if env_path && !env_path.empty?
    # An explicitly configured path must exist and parse (spec §2).
    begin
      data = JSON.parse(File.read(env_path))
    rescue StandardError => e
      abort("FATAL: BENCH_DATA_PATH=#{env_path} could not be loaded: #{e}")
    end
    source = env_path
  else
    data = nil
    source = nil
    ['/opt/bench/bench-data.json',
     File.expand_path('../shared/bench-data.json', __dir__)].each do |p|
      next unless File.exist?(p)
      begin
        data = JSON.parse(File.read(p))
      rescue StandardError => e
        abort("FATAL: bench-data.json exists at #{p} but could not be loaded: #{e}")
      end
      source = p
      break
    end
    if data.nil?
      abort('FATAL: bench-data.json not found (set BENCH_DATA_PATH or deploy ' \
            '/opt/bench/bench-data.json); reference implementations have no ' \
            'PRNG fallback (spec §2)')
    end
  end

  # Verify the §2 schema counts.
  {
    '_version == 2' => data['_version'] == 2,
    'users == 100' => (data['users']&.size == 100),
    'search_corpus == 1000' => (data['search_corpus']&.size == 1000),
    'timeseries == 10000' => (data['timeseries']&.size == 10_000),
    'transform_inputs == 10' => (data['transform_inputs']&.size == 10),
    'expected_checksums == 4 keys' => (data['expected_checksums']&.size == 4),
  }.each do |check, ok|
    abort("FATAL: bench-data.json at #{source}: schema check failed (#{check})") unless ok
  end

  LOGGER.info("Loaded bench-data.json from #{source} " \
              '(_version 2, 100 users, 1000 corpus, 10000 timeseries)')
  data
end

BENCH_DATA = load_bench_data!
USERS = BENCH_DATA['users'].freeze
SEARCH_CORPUS = BENCH_DATA['search_corpus'].freeze
# Aggregate reads only the value field, in dataset order (spec §5.6).
TS_VALUES = BENCH_DATA['timeseries'].map { |p| p['value'] }.freeze
EXPECTED_CHECKSUMS = BENCH_DATA['expected_checksums'].freeze

# /health body is a byte-constant precomputed at startup (spec §5.1).
HEALTH_BODY = JSON.generate(
  { 'status' => 'ok', 'runtime' => 'ruby', 'version' => RUBY_VERSION }
).freeze

USER_SORT_FIELDS = %w[id name email score created_at].freeze

# ── Helpers ──────────────────────────────────────────────────────────────────

def mono_ms
  Process.clock_gettime(Process::CLOCK_MONOTONIC) * 1000.0
end

# Spec §5.6: round half away from zero to 2 decimals (float64 semantics).
def r2(x)
  (x * 100 + 0.5).floor / 100.0
end

def api_headers(duration_ms)
  {
    'content-type'                => 'application/json',
    'server-timing'               => format('app;dur=%.1f', duration_ms),
    'cache-control'               => 'no-store, no-cache, must-revalidate',
    'timing-allow-origin'         => '*',
    'access-control-allow-origin' => '*',
  }
end

def api_json(body, duration_ms, status = 200)
  [status, api_headers(duration_ms), [JSON.generate(body)]]
end

def parse_query(qs)
  return {} if qs.nil? || qs.empty?
  URI.decode_www_form(qs).to_h
rescue ArgumentError
  {}
end

def int_param(params, key, default)
  Integer(params[key], 10)
rescue ArgumentError, TypeError
  default
end

# ── Main Rack app ────────────────────────────────────────────────────────────

app = proc do |env|
  path = env['PATH_INFO']
  method = env['REQUEST_METHOD']
  # HEAD is served by the GET handler with the body stripped (the validator's
  # header checks use HEAD, mirroring axum/starlette auto-HEAD behaviour).
  is_head = method == 'HEAD'
  method = 'GET' if is_head
  params = parse_query(env['QUERY_STRING'])

  # Bearer token authentication — every route except /health (spec §1).
  unless path == '/health' || BENCH_API_TOKEN.empty?
    auth = env['HTTP_AUTHORIZATION'] || ''
    unless auth == "Bearer #{BENCH_API_TOKEN}"
      next [401, { 'content-type' => 'application/json' }, ['{"error":"unauthorized"}']]
    end
  end

  response = case [method, path]
  when ['GET', '/health']
    # Constant-work: precomputed byte-constant body (spec §5.1).
    [200, { 'content-type' => 'application/json' }, [HEALTH_BODY]]

  when proc { |(m, p)| m == 'GET' && p.start_with?('/download/') }
    t0 = mono_ms
    size_str = path.delete_prefix('/download/')
    if size_str.match?(/\A\d+\z/) # non-integer → 400 (spec §5.2)
      size = [size_str.to_i, DOWNLOAD_CAP].min # clamp above cap; 0 is valid
      body = Enumerator.new do |yielder|
        remaining = size
        while remaining > 0
          to_send = [remaining, CHUNK_SIZE].min
          yielder << CHUNK.byteslice(0, to_send)
          remaining -= to_send
        end
      end
      headers = {
        'content-type'     => 'application/octet-stream',
        'content-length'   => size.to_s,
        'x-download-bytes' => size.to_s,
        'server-timing'    => format('proc;dur=%.1f', mono_ms - t0),
      }
      [200, headers, body]
    else
      [400, { 'content-type' => 'application/json' }, ['{"error":"invalid size"}']]
    end

  when ['POST', '/upload']
    t0 = mono_ms
    input = env['rack.input']
    total = 0
    while (chunk = input.read(CHUNK_SIZE))
      total += chunk.bytesize
    end
    headers = {
      'content-type'               => 'application/json',
      'x-networker-received-bytes' => total.to_s,
      'server-timing'              => format('recv;dur=%.1f', mono_ms - t0),
    }
    request_id = env['HTTP_X_NETWORKER_REQUEST_ID']
    headers['x-networker-request-id'] = request_id if request_id
    [200, headers, [%({"received_bytes":#{total}})]]

  # ── JSON API endpoints (family C, spec §5.4-§5.10) ──────────────────────

  when ['GET', '/api/users']
    t0 = mono_ms
    page = [int_param(params, 'page', 1), 1].max
    sort_field = params['sort'] || 'id'
    sort_field = 'id' unless USER_SORT_FIELDS.include?(sort_field)
    desc = params['order'] == 'desc'

    start = (page - 1) * 100
    window = USERS[start, 100] || []
    # Stable sort with dataset order breaking ties; desc reverses the
    # comparator (ties stay in dataset order, matching family C).
    sorted = window.each_with_index.sort do |(a, ai), (b, bi)|
      c = a[sort_field] <=> b[sort_field]
      c = -c if desc
      c.zero? ? ai <=> bi : c
    end.map(&:first)

    api_json(sorted.first(20), mono_ms - t0)

  when ['POST', '/api/transform']
    t0 = mono_ms
    raw = env['rack.input'].read || ''
    begin
      body = JSON.parse(raw)
      raise JSON::ParserError, 'not an object' unless body.is_a?(Hash)
    rescue JSON::ParserError
      next api_json({ 'error' => 'invalid JSON' }, mono_ms - t0, 400)
    end

    fields = body['fields'] || []
    values = body['values'] || []
    api_json({
      'seed'            => body['seed'] || 0,
      'hashed_fields'   => fields.map { |f| Digest::SHA256.hexdigest(f.to_s) },
      'reversed_values' => values.reverse,
    }, mono_ms - t0)

  when ['GET', '/api/aggregate']
    # `range` accepted and ignored (spec §5.6): full series, float64 math.
    t0 = mono_ms
    values = TS_VALUES.sort
    n = values.size
    total = 0.0
    values.each { |v| total += v } # sequential sum over SORTED values
    chunk = n / 5
    categories = (0...5).map do |i|
      part = values[i * chunk, chunk]
      s = 0.0
      part.each { |v| s += v }
      {
        'category' => "q#{i + 1}",
        'count'    => chunk,
        'mean'     => r2(s / chunk),
        'min'      => r2(part.first),
        'max'      => r2(part.last),
      }
    end
    api_json({
      'total_points' => n,
      'mean'         => r2(total / n),
      'p50'          => r2(values[(n * 0.50).to_i]),
      'p95'          => r2(values[(n * 0.95).to_i]),
      'max'          => r2(values.last),
      'categories'   => categories,
    }, mono_ms - t0)

  when ['GET', '/api/search']
    t0 = mono_ms
    query = params['q'] || 'test'
    limit = [int_param(params, 'limit', 20), 100].min

    # Case-sensitive regex; literal substring fallback on invalid pattern.
    begin
      pattern = Regexp.new(query)
      find = ->(item) { m = pattern.match(item); m&.begin(0) }
    rescue RegexpError
      find = ->(item) { item.index(query) }
    end

    matches = []
    SEARCH_CORPUS.each do |item|
      pos = find.call(item)
      matches << [pos, item] if pos
    end
    matches.sort! # [position asc, item asc bytewise]

    results = matches.first([limit, 0].max).each_with_index.map do |(pos, item), i|
      { 'rank' => i + 1, 'item' => item, 'match_position' => pos }
    end
    api_json({
      'query'         => query,
      'total_matches' => matches.size, # counted BEFORE truncation
      'returned'      => results.size,
      'results'       => results,
    }, mono_ms - t0)

  when ['POST', '/api/upload/process']
    t0 = mono_ms
    body = env['rack.input'].read || ''
    body = body.b
    api_json({
      'original_size'   => body.bytesize,
      'compressed_size' => Zlib::Deflate.deflate(body, 6).bytesize, # zlib RFC 1950, level 6
      'crc32'           => format('%08x', Zlib.crc32(body) & 0xFFFFFFFF),
      'sha256'          => Digest::SHA256.hexdigest(body),
    }, mono_ms - t0)

  when ['GET', '/api/delayed']
    t0 = mono_ms
    ms = int_param(params, 'ms', 10).clamp(1, 100)
    # `work` is reserved: accepted and ignored (spec §5.9).
    # Puma's concurrency model is threads (5:5 per worker); sleep parks the
    # handling thread, which is this runtime's documented timer semantics.
    sleep(ms / 1000.0)
    actual_ms = mono_ms - t0
    api_json({ 'requested_ms' => ms, 'actual_ms' => actual_ms.round(2) }, actual_ms)

  when ['GET', '/api/validate']
    t0 = mono_ms
    seed = int_param(params, 'seed', 42)
    api_json({ 'seed' => seed, 'checksums' => EXPECTED_CHECKSUMS }, mono_ms - t0)

  else
    [404, { 'content-type' => 'application/json' }, ['{"error":"not found"}']]
  end

  response = [response[0], response[1], []] if is_head
  response
end

run app
