# Puma configuration for AletheBench Ruby reference API.
#
# Worker policy (API-SPEC.md §3): BENCH_WORKERS cluster workers
# (default = logical CPU count), 5:5 threads per worker.

require 'etc'

cert_dir = ENV.fetch('BENCH_CERT_DIR', '/opt/bench')
port = ENV.fetch('BENCH_PORT', '8443')

bind "ssl://0.0.0.0:#{port}?cert=#{cert_dir}/cert.pem&key=#{cert_dir}/key.pem&verify_mode=none"

workers Integer(ENV.fetch('BENCH_WORKERS') { Etc.nprocessors })
threads 5, 5

# Load the app (and the shared dataset) once in the master before forking —
# dataset-load failures abort startup instead of crash-looping workers.
preload_app!

# Quiet startup logs.
environment 'production'
