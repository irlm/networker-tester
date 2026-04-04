#!/usr/bin/env bash
# Deploy .NET Framework 4.8 benchmark server on Windows Server
# Called by install.sh --benchmark-server csharp-net48
set -euo pipefail

BENCH_DIR="${BENCH_DIR:-/opt/bench}"
CERT_DIR="${BENCH_CERT_DIR:-$BENCH_DIR}"
PORT="${BENCH_PORT:-8443}"

echo ">> Compiling .NET 4.8 server"
# csc.exe is at the .NET Framework path on Windows
CSC="/c/Windows/Microsoft.NET/Framework64/v4.0.30319/csc.exe"
if [ ! -f "$CSC" ]; then
    CSC="$(find /c/Windows/Microsoft.NET/Framework64 -name csc.exe 2>/dev/null | head -1)"
fi

"$CSC" /out:"$BENCH_DIR/csharp-net48.exe" \
    /reference:System.IO.Compression.dll \
    /reference:System.Web.Extensions.dll \
    /target:exe \
    "$(dirname "$0")/Server.cs" || {
        echo "ERROR: csc.exe compilation failed"
        exit 1
    }

echo ">> Setting up HTTPS binding"
# Import cert and bind to port
THUMBPRINT=$(powershell -Command "
    \$cert = New-SelfSignedCertificate -DnsName 'bench' -CertStoreLocation Cert:\LocalMachine\My
    \$cert.Thumbprint
" 2>/dev/null || echo "")

if [ -n "$THUMBPRINT" ]; then
    netsh http add urlacl url="https://+:${PORT}/" user=Everyone 2>/dev/null || true
    netsh http add sslcert ipport=0.0.0.0:${PORT} certhash="$THUMBPRINT" appid="{12345678-1234-1234-1234-123456789012}" 2>/dev/null || true
fi

echo ">> Starting .NET 4.8 server on port $PORT"
nohup "$BENCH_DIR/csharp-net48.exe" "$PORT" "$CERT_DIR" > /dev/null 2>&1 &
echo ">> csharp-net48 server started (PID $!)"
