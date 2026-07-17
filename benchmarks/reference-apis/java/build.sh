#!/usr/bin/env bash
# Build the Java reference API into a runnable JAR.
# Requires JDK 21+.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "Compiling Server.java ..."
javac Server.java

echo "Packaging server.jar ..."
# Pack ALL compiled classes — javac emits one .class file per inner handler
# class (APIUsersHandler, AggregateHandler, ...); listing only three of them
# made the jar boot-fail with NoClassDefFoundError (audit V7).
jar cfe server.jar Server *.class

echo "Built: server.jar ($(du -h server.jar | cut -f1))"
