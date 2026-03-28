#!/usr/bin/env bash
# Build the Java reference API into a runnable JAR.
# Requires JDK 21+.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "Compiling Server.java ..."
javac Server.java

echo "Packaging server.jar ..."
jar cfe server.jar Server Server.class 'Server$HealthHandler.class' \
    'Server$DownloadHandler.class' 'Server$UploadHandler.class'

echo "Built: server.jar ($(du -h server.jar | cut -f1))"
