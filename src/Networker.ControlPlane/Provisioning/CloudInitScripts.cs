using System.Text.RegularExpressions;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// First-boot bootstrap script generation — the C# port of the Rust
/// <c>crates/networker-dashboard/src/services/cloud_init.rs</c> pure functions.
///
/// <para>These are <b>pure</b> string builders (no DB, no IO): given a dashboard
/// URL, an agent API key, and a target triple, they emit the exact bash
/// (cloud-init user-data) / PowerShell (AWS user-data / Azure custom-data) script
/// that provisions a fresh VM to run <c>networker-agent</c> (the self-contained
/// C# <c>Networker.Agent</c> since v0.28.26; the legacy Rust asset remains the
/// download fallback for older releases). Inputs are
/// whitelist-validated to prevent shell injection, matching the Rust regexes
/// byte-for-byte. The templates below are copied verbatim from the Rust
/// <c>LINUX_TEMPLATE</c> / <c>WINDOWS_TEMPLATE</c>; only the three placeholders
/// (<c>__TARGET_TRIPLE__</c>, <c>__DASHBOARD_URL__</c>, <c>__API_KEY__</c>) are
/// substituted.</para>
///
/// <para><b>Wiring note:</b> the real provisioners (<c>CliComputeProvisioner</c>)
/// pass this output as cloud user-data / custom-data. A later pass can call these
/// helpers there; this port only supplies the pure generators.</para>
/// </summary>
public static class CloudInitScripts
{
    // Rust: target_triple ^[A-Za-z0-9_-]+$
    private static readonly Regex TargetTripleRe = new("^[A-Za-z0-9_-]+$", RegexOptions.Compiled);

    // Rust: dashboard_url ^(https?|wss?)://[A-Za-z0-9.\-:]+(/[A-Za-z0-9._\-/]*)?$
    private static readonly Regex DashboardUrlRe =
        new(@"^(https?|wss?)://[A-Za-z0-9.\-:]+(/[A-Za-z0-9._\-/]*)?$", RegexOptions.Compiled);

    // Rust: api_key ^[A-Za-z0-9]{32,128}$
    private static readonly Regex ApiKeyRe = new("^[A-Za-z0-9]{32,128}$", RegexOptions.Compiled);

    /// <summary>
    /// Validate the three inputs (Rust <c>validate_inputs</c>). Throws
    /// <see cref="ArgumentException"/> with the exact Rust error message on the
    /// first failure (order: dashboard_url, api_key, target_triple).
    /// </summary>
    public static void ValidateInputs(string dashboardUrl, string apiKey, string targetTriple)
    {
        if (!DashboardUrlRe.IsMatch(dashboardUrl))
        {
            throw new ArgumentException(
                "invalid dashboard_url: must match https?://<host>[/path] with no shell metacharacters");
        }

        if (!ApiKeyRe.IsMatch(apiKey))
        {
            throw new ArgumentException("invalid api_key: must be 32-128 alphanumeric characters");
        }

        if (!TargetTripleRe.IsMatch(targetTriple))
        {
            throw new ArgumentException("invalid target_triple: must match [A-Za-z0-9_-]+");
        }
    }

    /// <summary>
    /// Derive the agent WebSocket URL from the public dashboard URL — the C# port
    /// of Rust <c>agent_ws_url</c>: trim a trailing <c>/</c>, map
    /// <c>https-&gt;wss</c> / <c>http-&gt;ws</c> (else unchanged), drop any path
    /// (agents always hit <c>/ws/agent</c>), and return
    /// <c>{scheme}://{host}/ws/agent</c>.
    /// </summary>
    public static string AgentWsUrl(string publicUrl)
    {
        var trimmed = publicUrl.TrimEnd('/');

        string scheme;
        string rest;
        if (trimmed.StartsWith("https://", StringComparison.Ordinal))
        {
            scheme = "wss";
            rest = trimmed["https://".Length..];
        }
        else if (trimmed.StartsWith("http://", StringComparison.Ordinal))
        {
            scheme = "ws";
            rest = trimmed["http://".Length..];
        }
        else if (trimmed.StartsWith("wss://", StringComparison.Ordinal))
        {
            scheme = "wss";
            rest = trimmed["wss://".Length..];
        }
        else if (trimmed.StartsWith("ws://", StringComparison.Ordinal))
        {
            scheme = "ws";
            rest = trimmed["ws://".Length..];
        }
        else
        {
            // No recognized scheme: leave as-is (Rust's "else unchanged" branch
            // operates on the scheme only; host is everything).
            scheme = string.Empty;
            rest = trimmed;
        }

        // Drop any path — keep only the host (up to the first '/').
        var slash = rest.IndexOf('/');
        var host = slash >= 0 ? rest[..slash] : rest;

        return string.IsNullOrEmpty(scheme) ? $"{host}/ws/agent" : $"{scheme}://{host}/ws/agent";
    }

    /// <summary>
    /// Render the Linux cloud-init bootstrap (Rust <c>render_linux_bootstrap</c>).
    /// Validates inputs, then substitutes the three placeholders into
    /// <see cref="LinuxTemplate"/>.
    /// </summary>
    public static string RenderLinuxBootstrap(string dashboardUrl, string apiKey, string targetTriple)
    {
        ValidateInputs(dashboardUrl, apiKey, targetTriple);
        return LinuxTemplate
            .Replace("__TARGET_TRIPLE__", targetTriple)
            .Replace("__DASHBOARD_URL__", dashboardUrl)
            .Replace("__API_KEY__", apiKey);
    }

    /// <summary>
    /// Render the Windows PowerShell bootstrap (Rust
    /// <c>render_windows_bootstrap</c>). Validates inputs, then substitutes the
    /// three placeholders into <see cref="WindowsTemplate"/>. The caller must wrap
    /// the output in <c>&lt;powershell&gt;...&lt;/powershell&gt;</c> for AWS
    /// user-data (Rust convention). The template is ASCII-only (Azure
    /// <c>--custom-data</c> latin-1 constraint).
    /// </summary>
    public static string RenderWindowsBootstrap(string dashboardUrl, string apiKey, string targetTriple)
    {
        ValidateInputs(dashboardUrl, apiKey, targetTriple);
        return WindowsTemplate
            .Replace("__TARGET_TRIPLE__", targetTriple)
            .Replace("__DASHBOARD_URL__", dashboardUrl)
            .Replace("__API_KEY__", apiKey);
    }

    /// <summary>
    /// Verbatim copy of the Rust <c>LINUX_TEMPLATE</c>. A verbatim string; the
    /// heredoc bodies and <c>__PLACEHOLDER__</c> tokens must stay byte-exact.
    /// </summary>
    public const string LinuxTemplate = """
#!/bin/bash
# Cloud-init bootstrap for networker-agent.
#
# Runs as root on first boot via cloud user-data (AWS), custom-data (Azure),
# or startup-script metadata (GCP).
#
# IMPORTANT: `set -e` is NOT used at the top level. A single transient apt
# hiccup or SIGPIPE from a pipeline would kill the whole bootstrap and the
# agent would never come online -- the #1 cause of "agent did not come online
# within 360s" on GCP (where startup-scripts run on shared-egress IPs that
# occasionally get GitHub-API-rate-limited). Instead, the critical agent
# install steps have their own explicit error handling, and a trap dumps
# context to the serial console so failures are diagnosable post-mortem.

# Log every command to the serial console so GCP/AWS/Azure consoles show
# exactly where a bootstrap got stuck. `logger -t` also fans it to journald.
exec > >(tee /var/log/networker-bootstrap.log | logger -t networker-bootstrap -s 2>/dev/console) 2>&1
set -x

trap 'rc=$?; echo "networker-bootstrap: exited with rc=$rc at line $LINENO" >&2' EXIT

export DEBIAN_FRONTEND=noninteractive

# 1. Prereqs (apt or dnf) + Wireshark CLI (tshark).
#    Retry apt-get update once on failure -- cloud-init frequently races the
#    unattended-upgrades apt lock, and a one-shot failure should not abort.
if command -v apt-get >/dev/null 2>&1; then
    # Wait up to 120s for any concurrent apt lock holder (cloud-init's own
    # unattended-upgrades, package-install hooks, etc) to finish.
    for _ in $(seq 1 24); do
        fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break
        echo "networker-bootstrap: waiting for apt lock..." >&2
        sleep 5
    done
    echo "wireshark-common wireshark-common/install-setuid boolean true" \
        | debconf-set-selections
    apt-get update -y -qq || (sleep 10; apt-get update -y -qq) || true
    apt-get install -y -qq curl tar ca-certificates tshark \
        || apt-get install -y -qq curl tar ca-certificates tshark \
        || { echo "networker-bootstrap: apt install curl/tar/tshark failed" >&2; exit 1; }
    # Chromium for Page Load (Browser) probes. Soft-fail: a missing browser
    # must not abort the bootstrap (agent still comes online, only browser
    # probes are degraded).
    apt-get install -y -qq chromium-browser || apt-get install -y -qq chromium || true
elif command -v dnf >/dev/null 2>&1; then
    dnf install -y curl tar ca-certificates wireshark-cli \
        || { echo "networker-bootstrap: dnf install failed" >&2; exit 1; }
    dnf install -y chromium || true
else
    echo "networker-bootstrap: no supported package manager (apt/dnf) found" >&2
    exit 1
fi

# 2. Allow non-root packet capture via dumpcap.
if [ -x /usr/bin/dumpcap ]; then
    setcap cap_net_raw,cap_net_admin=eip /usr/bin/dumpcap || true
elif [ -x /usr/sbin/dumpcap ]; then
    setcap cap_net_raw,cap_net_admin=eip /usr/sbin/dumpcap || true
fi

# 3. Resolve the latest release tag. Uses `grep -m1` instead of `| head -1`
#    because `set -o pipefail` + `head -1` races grep and trips SIGPIPE (exit
#    141) on small responses -- an intermittent failure mode that silently
#    blanked $TAG on GCP us-central1 (shared egress, occasional slow github
#    response). `grep -m1` makes grep itself stop after the first match so
#    there is no pipe-close race. Retry the API call up to 5x (GitHub
#    rate-limits unauthenticated calls from shared cloud egress IPs).
TARGET="__TARGET_TRIPLE__"
TAG=""
for attempt in 1 2 3 4 5; do
    TAG=$(curl -fsSL --retry 3 --retry-delay 3 --max-time 30 \
        -H 'Accept: application/vnd.github+json' \
        https://api.github.com/repos/irlm/networker-tester/releases/latest \
        | grep -m1 '"tag_name":' \
        | cut -d'"' -f4 || true)
    if [ -n "$TAG" ]; then break; fi
    echo "networker-bootstrap: GitHub API attempt $attempt returned empty tag; retrying..." >&2
    sleep $((attempt * 5))
done
if [ -z "$TAG" ]; then
    echo "networker-bootstrap: could not resolve latest release tag after 5 attempts" >&2
    exit 1
fi
echo "networker-bootstrap: resolved TAG=$TAG TARGET=$TARGET"

download_bin() {
    BIN="$1"
    URL="https://github.com/irlm/networker-tester/releases/download/${TAG}/${BIN}-${TARGET}.tar.gz"
    # --retry-connrefused handles VMs that haven't finished DNS/network bring-up
    # yet. --retry-all-errors (curl >= 7.71) retries on every HTTP failure too.
    curl -fsSL --retry 5 --retry-delay 3 --retry-connrefused --max-time 180 \
        "$URL" -o "/tmp/${BIN}.tar.gz" \
        || { echo "networker-bootstrap: failed to download $URL" >&2; return 1; }
    tar xzf "/tmp/${BIN}.tar.gz" -C /tmp \
        || { echo "networker-bootstrap: failed to extract ${BIN}.tar.gz" >&2; return 1; }
    install -m 0755 "/tmp/${BIN}" "/usr/local/bin/${BIN}" \
        || { echo "networker-bootstrap: failed to install /usr/local/bin/${BIN}" >&2; return 1; }
    rm -f "/tmp/${BIN}.tar.gz" "/tmp/${BIN}"
}

# C# agent download: the agent is the self-contained C# Networker.Agent
# (published from the ubuntu runner as networker-agent-cs-linux-x64.tar.gz;
# the binary inside is still named networker-agent -- drop-in). Falls back to
# the legacy Rust asset name so a bootstrap that resolves an OLDER release
# (predating the -cs- assets) still provisions.
download_agent() {
    URL="https://github.com/irlm/networker-tester/releases/download/${TAG}/networker-agent-cs-linux-x64.tar.gz"
    curl -fsSL --retry 5 --retry-delay 3 --retry-connrefused --max-time 180 \
        "$URL" -o /tmp/networker-agent.tar.gz \
        || { echo "networker-bootstrap: C# agent asset unavailable at $URL; falling back to legacy Rust agent" >&2; \
             download_bin networker-agent; return $?; }
    tar xzf /tmp/networker-agent.tar.gz -C /tmp \
        || { echo "networker-bootstrap: failed to extract networker-agent.tar.gz" >&2; return 1; }
    install -m 0755 /tmp/networker-agent /usr/local/bin/networker-agent \
        || { echo "networker-bootstrap: failed to install /usr/local/bin/networker-agent" >&2; return 1; }
    rm -f /tmp/networker-agent.tar.gz /tmp/networker-agent
}

# Literal asset names (also assert tests can grep for):
#   networker-tester-__TARGET_TRIPLE__.tar.gz
#   networker-agent-cs-linux-x64.tar.gz (fallback: networker-agent-__TARGET_TRIPLE__.tar.gz)
download_bin networker-tester || exit 1
download_agent || exit 1

# 4. systemd unit (system-level, runs on boot, survives SSH disconnect).
mkdir -p /etc/systemd/system
cat > /etc/systemd/system/networker-agent.service <<'UNIT'
[Unit]
Description=Networker Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/networker-agent
Restart=on-failure
RestartSec=5
Environment=AGENT_DASHBOARD_URL=__DASHBOARD_URL__
Environment=AGENT_API_KEY=__API_KEY__

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now networker-agent.service \
    || { echo "networker-bootstrap: systemctl enable --now failed" >&2; systemctl status networker-agent.service --no-pager || true; exit 1; }
echo "networker-bootstrap: networker-agent installed and started"
""";

    /// <summary>
    /// Verbatim copy of the Rust <c>WINDOWS_TEMPLATE</c>. Guaranteed ASCII-only
    /// (Azure custom-data latin-1 constraint — do not introduce non-ASCII).
    /// </summary>
    public const string WindowsTemplate = """
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Force TLS 1.2 for the whole .NET process. Windows 11 Desktop 24H2 images
# still default the .NET Framework WebClient / ServicePointManager to
# Ssl3|Tls, and chocolatey.org + GitHub API reject anything below TLS 1.2.
# Without this, `System.Net.WebClient.DownloadString` on a fresh Win11 Pro
# VM fails the TLS handshake and CSE reports "VM has reported a ...failure
# when processing extension 'CustomScriptExtension'". Windows Server 2022
# was unaffected because it enables strong crypto at the SChannel layer.
[System.Net.ServicePointManager]::SecurityProtocol = `
    [System.Net.ServicePointManager]::SecurityProtocol -bor [System.Net.SecurityProtocolType]::Tls12

# 1. Install Chocolatey (idempotent)
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Set-ExecutionPolicy Bypass -Scope Process -Force
    iex ((New-Object System.Net.WebClient).DownloadString('https://chocolatey.org/install.ps1'))
}

# 2. Wireshark + Npcap (loopback-capable, no WinPcap mode)
# Npcap was removed from the Chocolatey community repository (licensing), so
# the install is soft-failed: packet capture degrades but the agent still
# comes online. Wireshark itself is still in the community repo.
# Wireshark install wrapped in try/catch so a transient Chocolatey / MSI
# driver-install failure (seen on Win11 Desktop 24H2 under strict SmartScreen
# / Defender defaults) does not abort the whole extension -- the agent comes
# online and only packet capture degrades.
try {
    choco install -y --no-progress wireshark --params '/NoDesktopIcon /NoQuickLaunchIcon'
} catch {
    Write-Warning "Wireshark install failed: $_"
}
try {
    choco install -y --no-progress npcap --params '/WinPcapMode=no /LoopbackSupport=yes'
} catch {
    Write-Warning "npcap install failed (package removed from Chocolatey community repo): $_"
}

# Chrome for Page Load (Browser) probes. Soft-fail so a missing browser
# does not abort the bootstrap -- the agent still comes online, only
# browser probes are degraded.
try {
    choco install -y --no-progress googlechrome --params '/NoDesktopIcon'
} catch {
    Write-Warning "Chrome install failed: $_"
}

# Add Wireshark to the machine PATH so tshark resolves from any service.
$wireshark = 'C:\Program Files\Wireshark'
if (Test-Path $wireshark) {
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if (-not ($machinePath -split ';' | Where-Object { $_ -ieq $wireshark })) {
        [Environment]::SetEnvironmentVariable('Path', "$machinePath;$wireshark", 'Machine')
    }
}

# 3. Resolve latest release tag and download binaries
$TARGET = '__TARGET_TRIPLE__'
$TAG = (Invoke-RestMethod 'https://api.github.com/repos/irlm/networker-tester/releases/latest').tag_name
if (-not $TAG) { throw 'could not resolve latest release tag' }

$BinDir = 'C:\Program Files\Networker'
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

# Literal asset names (also assert tests can grep for):
#   networker-tester-__TARGET_TRIPLE__.zip
#   networker-agent-cs-win-x64.zip (fallback: networker-agent-__TARGET_TRIPLE__.zip)
# Windows release artefacts are zipped. Unpacked with Expand-Archive (native
# on Windows, no tar shim needed).
function Install-NetworkerZip($name, $url) {
    $zip = "$env:TEMP\$name.zip"
    $extract = "$env:TEMP\$name-extract"
    Invoke-WebRequest -Uri $url -OutFile $zip
    if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
    Expand-Archive -Path $zip -DestinationPath $extract -Force
    Copy-Item -Force "$extract\$name.exe" "$BinDir\$name.exe"
    Remove-Item -Force $zip
    Remove-Item -Recurse -Force $extract
}
Install-NetworkerZip 'networker-tester' "https://github.com/irlm/networker-tester/releases/download/$TAG/networker-tester-$TARGET.zip"
# The agent is the self-contained C# Networker.Agent (published from the
# ubuntu runner; the exe inside is still networker-agent.exe -- drop-in).
# Fall back to the legacy Rust asset if the resolved release predates it.
try {
    Install-NetworkerZip 'networker-agent' "https://github.com/irlm/networker-tester/releases/download/$TAG/networker-agent-cs-win-x64.zip"
} catch {
    Write-Host 'networker-bootstrap: C# agent asset unavailable; falling back to legacy Rust agent'
    Install-NetworkerZip 'networker-agent' "https://github.com/irlm/networker-tester/releases/download/$TAG/networker-agent-$TARGET.zip"
}

# 4. Set machine env vars + install service via sc.exe
[Environment]::SetEnvironmentVariable('AGENT_DASHBOARD_URL', '__DASHBOARD_URL__', 'Machine')
[Environment]::SetEnvironmentVariable('AGENT_API_KEY', '__API_KEY__', 'Machine')

sc.exe stop NetworkerAgent 2>$null | Out-Null
sc.exe delete NetworkerAgent 2>$null | Out-Null
sc.exe create NetworkerAgent binPath= "`"$BinDir\networker-agent.exe`"" start= auto DisplayName= "Networker Agent" | Out-Null

# Restart so the service inherits the new env vars
Restart-Service NetworkerAgent -ErrorAction SilentlyContinue
if ((Get-Service NetworkerAgent).Status -ne 'Running') {
    Start-Service NetworkerAgent
}

Write-Host 'networker-agent installed and started'
""";
}
