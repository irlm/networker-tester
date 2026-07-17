# Puma configuration for Networker Bench Ruby reference API.
#
# Worker policy (API-SPEC.md §3): BENCH_WORKERS cluster workers
# (default = logical CPU count), 5:5 threads per worker.

require 'etc'

cert_dir = ENV.fetch('BENCH_CERT_DIR', '/opt/bench')
port = ENV.fetch('BENCH_PORT', '8443')

# Listener type is chosen at startup from cert presence (audit F8): certs
# absent → plain HTTP on the same port (application mode behind a
# TLS-terminating reverse proxy), mirroring the Go/Node/Java pattern.
cert = "#{cert_dir}/cert.pem"
key = "#{cert_dir}/key.pem"
if File.file?(cert) && File.file?(key)
  bind "ssl://0.0.0.0:#{port}?cert=#{cert}&key=#{key}&verify_mode=none"
else
  warn "no TLS certs in #{cert_dir} - serving plain HTTP on port #{port} (application mode)"
  bind "tcp://0.0.0.0:#{port}"
end

workers Integer(ENV.fetch('BENCH_WORKERS') { Etc.nprocessors })
threads 5, 5

# Load the app (and the shared dataset) once in the master before forking —
# dataset-load failures abort startup instead of crash-looping workers.
preload_app!

# Quiet startup logs.
environment 'production'
