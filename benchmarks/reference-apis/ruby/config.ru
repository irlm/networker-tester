# AletheBench Ruby reference API — direct Rack app on Puma.

require 'digest/sha2'
require 'json'
require 'logger'
require 'zlib'

LOGGER = Logger.new($stderr)
LOGGER.level = ENV.fetch('LOG_LEVEL', 'INFO').upcase == 'DEBUG' ? Logger::DEBUG : Logger::INFO

CHUNK_SIZE = 8192
CHUNK = ("\x42" * CHUNK_SIZE).b.freeze

# ── Shared benchmark dataset ────────────────────────────────────────────────

BENCH_DATA = begin
  paths = []
  paths << ENV['BENCH_DATA_PATH'] if ENV['BENCH_DATA_PATH']
  paths << '/opt/bench/bench-data.json'
  paths << File.expand_path('../shared/bench-data.json', __dir__)

  loaded = nil
  paths.each do |p|
    next unless File.exist?(p)
    begin
      loaded = JSON.parse(File.read(p))
      LOGGER.info("Loaded bench-data.json from #{p} (version #{loaded['_version']}, " \
                  "#{loaded['users']&.size} users, #{loaded['search_corpus']&.size} corpus, " \
                  "#{loaded['timeseries']&.size} timeseries)")
      break
    rescue => e
      LOGGER.warn("bench-data.json at #{p} is invalid: #{e}")
    end
  end
  unless loaded
    LOGGER.warn("bench-data.json not found, falling back to per-language PRNG")
  end
  loaded
end

# ── Shared data for API endpoints ───────────────────────────────────────────

FIRST_NAMES = %w[
  Alice Bob Carol Dave Eve Frank Grace Heidi
  Ivan Judy Karl Laura Mallory Nina Oscar Peggy
  Quentin Ruth Steve Trent Ursula Victor Wendy
  Xander Yvonne Zack
].freeze

LAST_NAMES = %w[
  Smith Johnson Williams Brown Jones Garcia Miller
  Davis Rodriguez Martinez Hernandez Lopez Gonzalez
  Wilson Anderson Thomas Taylor Moore Jackson Martin
].freeze

DEPARTMENTS = %w[
  Engineering Marketing Sales Finance HR
  Operations Legal Support Design Product
].freeze

SEARCH_WORDS = %w[
  network latency throughput bandwidth packet
  routing firewall proxy endpoint server
  client protocol socket buffer stream
  timeout retry cache queue load
].freeze

# ── Helpers ──────────────────────────────────────────────────────────────────

def api_headers(duration_ms)
  {
    "content-type"                => "application/json",
    "server-timing"               => "app;dur=#{format('%.1f', duration_ms)}",
    "cache-control"               => "no-store, no-cache, must-revalidate",
    "timing-allow-origin"         => "*",
    "access-control-allow-origin" => "*",
  }
end

def api_json(body, duration_ms, status = "200")
  [status, api_headers(duration_ms), [JSON.generate(body)]]
end

def parse_query(qs)
  return {} if qs.nil? || qs.empty?
  qs.split("&").each_with_object({}) do |pair, h|
    k, v = pair.split("=", 2)
    h[k] = v || ""
  end
end

def generate_users(rng, count = 100)
  (1..count).map do |i|
    {
      id: i,
      name: "#{FIRST_NAMES[rng.rand(FIRST_NAMES.size)]} #{LAST_NAMES[rng.rand(LAST_NAMES.size)]}",
      email: "user#{i}@example.com",
      age: rng.rand(22..65),
      department: DEPARTMENTS[rng.rand(DEPARTMENTS.size)],
      score: (rng.rand * 100).round(2),
    }
  end
end

# ── Main Rack app ────────────────────────────────────────────────────────────

app = proc do |env|
  path = env["PATH_INFO"]
  method = env["REQUEST_METHOD"]
  params = parse_query(env["QUERY_STRING"])

  case [method, path]
  when ["GET", "/health"]
    body = %({"status":"ok","runtime":"ruby","version":"#{RUBY_VERSION}"})
    ["200", { "content-type" => "application/json" }, [body]]

  when ->(_, p) { method == "GET" && p.match?(%r{\A/download/\d+\z}) }
    size = path.split("/").last.to_i
    if size <= 0
      ["400", { "content-type" => "application/json" }, ['{"error":"invalid size"}']]
    else
      headers = {
        "content-type" => "application/octet-stream",
        "content-length" => size.to_s,
      }
      body = Enumerator.new do |yielder|
        remaining = size
        while remaining > 0
          to_send = [remaining, CHUNK_SIZE].min
          yielder << CHUNK.byteslice(0, to_send)
          remaining -= to_send
        end
      end
      ["200", headers, body]
    end

  when ["POST", "/upload"]
    input = env["rack.input"]
    total = 0
    while (chunk = input.read(CHUNK_SIZE))
      total += chunk.bytesize
    end
    body = %({"bytes_received":#{total}})
    ["200", { "content-type" => "application/json" }, [body]]

  # ── JSON API endpoints ──────────────────────────────────────────────────

  when ["GET", "/api/users"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    page = (params["page"] || "1").to_i
    sort_field = params["sort"] || "id"
    order = params["order"] || "asc"

    if BENCH_DATA && BENCH_DATA["users"]
      users = BENCH_DATA["users"].map { |u| u.transform_keys(&:to_sym) }
    else
      rng = Random.new(page)
      users = generate_users(rng)
    end

    valid_fields = %w[id name email age department score]
    if valid_fields.include?(sort_field)
      users.sort_by! { |u| u[sort_field.to_sym] }
      users.reverse! if order == "desc"
    end

    page_size = 20
    start = (page - 1) * page_size
    page_users = users[start, page_size] || []

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      page: page,
      page_size: page_size,
      total: users.size,
      sort: sort_field,
      order: order,
      users: page_users,
    }, duration_ms)

  when ["POST", "/api/transform"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    input = env["rack.input"]
    raw = input.read || ""
    begin
      body = JSON.parse(raw)
    rescue JSON::ParserError
      duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
      next api_json({ error: "invalid JSON" }, duration_ms, "400")
    end

    transformed = {}
    body.each do |key, value|
      if value.is_a?(String)
        hashed = Digest::SHA256.hexdigest(value)
        transformed[key] = { original_reversed: value.reverse, sha256: hashed }
      else
        transformed[key] = value
      end
    end

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      original_fields: body.size,
      transformed: transformed,
    }, duration_ms)

  when ["GET", "/api/aggregate"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    range_param = params["range"] || "0,1000"
    parts = range_param.split(",")
    range_start = parts[0].to_i
    range_end = parts.size >= 2 ? parts[1].to_i : 1000

    if BENCH_DATA && BENCH_DATA["timeseries"]
      ts = BENCH_DATA["timeseries"]
      count = ts.size
      values = ts.map { |p| p["value"] }
      categories = %w[alpha beta gamma delta epsilon]
      assignments = ts.map { |p| p["category"] }
    else
      rng = Random.new(range_start)
      count = 10000
      # Box-Muller for Gaussian
      values = Array.new(count) do
        u1 = rng.rand
        u2 = rng.rand
        z = Math.sqrt(-2.0 * Math.log(u1)) * Math.cos(2.0 * Math::PI * u2)
        50 + 15 * z
      end

      categories = %w[alpha beta gamma delta epsilon]
      cat_rng = Random.new(range_start + 1)
      assignments = Array.new(count) { categories[cat_rng.rand(categories.size)] }
    end

    sorted_vals = values.sort
    mean = values.sum / count.to_f
    p50 = sorted_vals[count / 2]
    p95 = sorted_vals[(count * 0.95).to_i]
    max_val = sorted_vals[-1]

    groups = {}
    categories.each do |cat|
      cat_vals = values.each_with_index.select { |_, i| assignments[i] == cat }.map(&:first)
      next if cat_vals.empty?
      cat_sorted = cat_vals.sort
      groups[cat] = {
        count: cat_vals.size,
        mean: (cat_vals.sum / cat_vals.size.to_f).round(4),
        p50: cat_sorted[cat_sorted.size / 2].round(4),
        max: cat_sorted[-1].round(4),
      }
    end

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      range: { start: range_start, end: range_end },
      total_points: count,
      stats: {
        mean: mean.round(4),
        p50: p50.round(4),
        p95: p95.round(4),
        max: max_val.round(4),
      },
      groups: groups,
    }, duration_ms)

  when ["GET", "/api/search"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    query = params["q"] || "test"
    limit = (params["limit"] || "10").to_i

    if BENCH_DATA && BENCH_DATA["search_corpus"]
      corpus = BENCH_DATA["search_corpus"].each_with_index.map do |text, i|
        { id: i + 1, text: text }
      end
    else
      rng = Random.new(42)
      corpus = (1..1000).map do |i|
        word_count = rng.rand(3..8)
        phrase = Array.new(word_count) { SEARCH_WORDS[rng.rand(SEARCH_WORDS.size)] }.join(" ")
        { id: i, text: phrase }
      end
    end

    begin
      pattern = Regexp.new(query, Regexp::IGNORECASE)
    rescue RegexpError
      duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
      next api_json({ error: "invalid regex" }, duration_ms, "400")
    end

    results = []
    corpus.each do |item|
      m = pattern.match(item[:text])
      if m
        score = 1.0 / (1 + m.begin(0))
        results << { id: item[:id], text: item[:text], score: score.round(4) }
      end
    end

    results.sort_by! { |r| -r[:score] }
    results = results[0, limit]

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      query: query,
      total_matches: results.size,
      limit: limit,
      results: results,
    }, duration_ms)

  when ["POST", "/api/upload/process"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    input = env["rack.input"]
    body = input.read || "".b
    body = body.b

    crc = Zlib.crc32(body) & 0xFFFFFFFF
    sha = Digest::SHA256.hexdigest(body)
    compressed = Zlib::Deflate.deflate(body)

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      original_size: body.bytesize,
      compressed_size: compressed.bytesize,
      compression_ratio: (compressed.bytesize.to_f / [body.bytesize, 1].max).round(4),
      crc32: format("%08x", crc),
      sha256: sha,
    }, duration_ms)

  when ["GET", "/api/delayed"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    ms = [[((params["ms"] || "100").to_i), 1].max, 100].min
    work = params["work"] || "none"

    sleep(ms / 1000.0)

    if work == "light"
      Digest::SHA256.hexdigest("benchmark" * 100)
    end

    actual_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      requested_ms: ms,
      actual_ms: actual_ms.round(2),
      work: work,
    }, actual_ms)

  when ["GET", "/api/validate"]
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    seed = (params["seed"] || "42").to_i

    if BENCH_DATA && BENCH_DATA["expected_checksums"]
      checksums = BENCH_DATA["expected_checksums"]
      users_hash = checksums["users_page1"] || ""
      agg_hash = checksums["aggregate_summary"] || ""
      search_hash = checksums["search_network_top10"] || ""
    else
      # Users checksum (page=1)
      rng = Random.new(1)
      users = generate_users(rng)
      users_hash = Digest::SHA256.hexdigest(JSON.generate(users.map { |u| u.sort_by { |k, _| k.to_s }.to_h }))

      # Aggregate checksum (start=0)
      rng = Random.new(0)
      values = Array.new(10000) do
        u1 = rng.rand
        u2 = rng.rand
        z = Math.sqrt(-2.0 * Math.log(u1)) * Math.cos(2.0 * Math::PI * u2)
        (50 + 15 * z).round(4)
      end.sort
      agg_hash = Digest::SHA256.hexdigest(JSON.generate(values))

      # Search checksum
      rng = Random.new(42)
      corpus = (1..1000).map do
        word_count = rng.rand(3..8)
        Array.new(word_count) { SEARCH_WORDS[rng.rand(SEARCH_WORDS.size)] }.join(" ")
      end
      search_hash = Digest::SHA256.hexdigest(JSON.generate(corpus))
    end

    duration_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000
    api_json({
      seed: seed,
      checksums: {
        users_page1: users_hash[0, 16],
        aggregate_start0: agg_hash[0, 16],
        search_corpus: search_hash[0, 16],
      },
    }, duration_ms)

  else
    ["404", { "content-type" => "application/json" }, ['{"error":"not found"}']]
  end
end

run app
