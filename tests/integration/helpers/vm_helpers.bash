#!/usr/bin/env bash
# Shared helpers for integration tests: SSH, VM health checks, assertions.

# ssh_run IP USER CMD — run CMD on remote VM, return output
ssh_run() {
    local ip="$1" user="$2"
    shift 2
    ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${user}@${ip}" "$@"
}

# wait_for_ssh IP USER [TIMEOUT_SECS] — poll until SSH is available
wait_for_ssh() {
    local ip="$1" user="$2" timeout="${3:-120}"
    local elapsed=0
    until ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 \
              "${user}@${ip}" true 2>/dev/null; do
        [[ $elapsed -ge $timeout ]] && { echo "SSH timeout on ${ip}"; return 1; }
        sleep 5; elapsed=$(( elapsed + 5 ))
    done
}

# assert_binary_version IP USER BINARY EXPECTED_VERSION
assert_binary_version() {
    local ip="$1" user="$2" binary="$3" expected="$4"
    local actual
    actual="$(ssh_run "$ip" "$user" "${binary} --version 2>/dev/null" || true)"
    [[ "$actual" == *"$expected"* ]] || {
        echo "FAIL: ${binary} version mismatch — got '${actual}', want '${expected}'"
        return 1
    }
}
