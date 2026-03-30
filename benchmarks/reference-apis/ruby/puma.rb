# Puma configuration for AletheBench Ruby reference API.

cert_dir = ENV.fetch("BENCH_CERT_DIR", "/opt/bench")

bind "ssl://0.0.0.0:8443?cert=#{cert_dir}/cert.pem&key=#{cert_dir}/key.pem&verify_mode=none"

# Single process, no cluster — fair comparison with other single-process servers.
workers 0

# Thread pool: 4 minimum, 16 maximum.
threads 4, 16

# Quiet startup logs.
environment "production"
