# AletheBench Ruby reference API — direct Rack app on Puma.

CHUNK_SIZE = 8192
CHUNK = ("\x42" * CHUNK_SIZE).b.freeze

app = proc do |env|
  path = env["PATH_INFO"]
  method = env["REQUEST_METHOD"]

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

  else
    ["404", { "content-type" => "application/json" }, ['{"error":"not found"}']]
  end
end

run app
