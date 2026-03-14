---
applyTo: "install.sh,install.ps1,tests/installer.bats,tests/stubs/**,tests/*.sh"
---

# Installer & Shell Script Rules

## Bash 3.2 Compatibility (install.sh)
- No associative arrays (`declare -A`) — use separate variables or indexed arrays.
- No `[[ -v VAR ]]` — use `[[ -n "${VAR:-}" ]]` or `[[ -z "${VAR:-}" ]]`.
- No nameref (`declare -n`) or indirect expansion (`${!ref}`).
- No `readarray`/`mapfile` — use `while IFS= read -r` loops.
- No `|&` (pipe stderr) — use `2>&1 |`.
- Rationale: macOS ships Bash 3.2; Alpine Linux may have older bash.

## curl|bash Stdin Protection
- Any command that reads stdin inside install.sh will consume the piped script. Use `< /dev/null` for non-interactive commands (ssh, az, aws, gcloud, apt-get).
- Interactive prompts must read from `< /dev/tty`.
- Background subshells that read stdin will silently break the install flow.

## Version Synchronization
- `INSTALLER_VERSION` fallback string must match in BOTH install.sh AND install.ps1.
- This value is used when `gh release list` fails (no gh CLI, rate-limited, etc.).
- Format: `"vX.Y.Z"` (with leading v).

## Windows Deployment (install.ps1)
- Never use `-NoNewWindow` for endpoint start — it keeps child attached to console and hangs `az vm run-command`.
- Always use `Start-Process -WindowStyle Hidden` for background processes.
- Use `schtasks /SC ONSTART` for persistence, not `sc.exe` (endpoint is not a Windows service).
- VM names must be ≤15 characters (NetBIOS/Windows hostname limit).
- Static CRT linking (`-C target-feature=+crt-static`) eliminates VC++ runtime dependency — do not add VC++ Redistributable install steps.

## Cloud CLI Safety
- Check tool availability with `command -v` (bash) or `Get-Command` (PowerShell) before calling cloud CLIs.
- Never execute `gcloud` during system discovery — Python startup adds 2-3s latency. Only `command -v gcloud`.
- Credential checks: `az account show`, `aws sts get-caller-identity`, `gcloud auth print-access-token`.
- All cloud CLI calls that might prompt interactively need `< /dev/null` in bash or `-Force`/`-Confirm:$false` in PowerShell.

## iptables Cleanup
- Linux endpoint VMs use `iptables -t nat REDIRECT` for port mapping.
- OUTPUT REDIRECT rules break ALL outbound HTTPS on the VM — always clean up with matching `-D` rules after install.
- Check existing rules before adding duplicates.

## Testing (bats)
- Installer tests use stubs in `tests/stubs/` that mock `gh`, `cargo`, `ssh`, `az`, `aws`, `gcloud`, `curl`.
- Stubs return canned responses — update them when adding new CLI interactions.
- Tests source install.sh functions directly; avoid testing with actual network calls.
- New deploy-config features need bats test coverage.
