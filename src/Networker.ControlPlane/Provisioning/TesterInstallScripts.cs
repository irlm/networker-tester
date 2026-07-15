namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// SSH-install command generation — the C# port of the pure script-building
/// parts of Rust <c>crates/networker-dashboard/src/services/tester_install.rs</c>
/// (the path that SSHes into a freshly-provisioned tester VM and installs the
/// pre-built <c>networker-tester</c> + <c>networker-agent</c> binaries from
/// GitHub releases, plus optional Chrome + chrome-harness).
///
/// <para><b>Scope of this port:</b> the <i>logic</i> — OS detection command,
/// package-manager selection + prereq command, per-tag download command, the
/// user-level systemd unit + install command, browser-harness command, verify
/// commands, and the release-tag / OS-target resolution. The <b>side effect</b>
/// (spawning <c>ssh</c>) is NOT performed here; that's a host-side action the
/// control plane can't do in CI. A later wiring pass runs these command strings
/// over an SSH transport. See <c>// TODO(phase3)</c> in <see cref="RunSshStub"/>.</para>
///
/// <para>Command strings are copied verbatim from the Rust source; only the
/// <c>{...}</c> format substitutions (tag, target, binary, env lines) are filled
/// in, matching the Rust <c>format!</c> calls exactly.</para>
/// </summary>
public static class TesterInstallScripts
{
    /// <summary>GitHub repo owner/name for release downloads (Rust hardcodes
    /// <c>irlm/networker-tester</c>).</summary>
    public const string Repo = "irlm/networker-tester";

    /// <summary>Env var: when set to anything other than <c>0</c> / <c>false</c>,
    /// Chrome install is SKIPPED. Default (unset / <c>0</c> / <c>false</c>) =
    /// install Chrome. Note the inverted semantics — faithful to Rust.</summary>
    public const string SkipChromeEnv = "DASHBOARD_TESTER_SKIP_CHROME";

    /// <summary>Env var: total SSH-wait seconds (u32), default 300, clamped
    /// [60, 900]. Rust: <c>DASHBOARD_TESTER_SSH_WAIT_SECS</c>.</summary>
    public const string SshWaitSecsEnv = "DASHBOARD_TESTER_SSH_WAIT_SECS";

    /// <summary>
    /// Whether to install Chrome, from <see cref="SkipChromeEnv"/>. Rust:
    /// <c>install_chrome = (value == "0" OR case-insensitive "false")</c>, default
    /// true. So: unset =&gt; true; <c>"0"</c>/<c>"false"</c> =&gt; true; any other
    /// value =&gt; false.
    /// </summary>
    public static bool ShouldInstallChrome(string? envValue)
    {
        if (envValue is null)
        {
            return true;
        }

        return envValue == "0" || string.Equals(envValue, "false", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>
    /// Parse + clamp the SSH-wait seconds. Rust: parse u32, default 300, clamp to
    /// [60, 900]. Returns the total seconds; attempts = total/5, 5s between.
    /// </summary>
    public static uint SshWaitSecs(string? envValue)
    {
        if (envValue is null || !uint.TryParse(envValue, out var v))
        {
            v = 300;
        }

        return Math.Clamp(v, 60u, 900u);
    }

    /// <summary>Rust <c>preferred_release_tag()</c> = <c>"v{version}"</c>. The
    /// dashboard version is the caller's compile-time version.</summary>
    public static string PreferredReleaseTag(string version) => $"v{version}";

    /// <summary>
    /// Rust <c>TesterOsInfo::release_target()</c>: musl static targets by arch,
    /// defaulting to x86_64.
    /// </summary>
    public static string ReleaseTarget(string arch) => arch switch
    {
        "x86_64" => "x86_64-unknown-linux-musl",
        "aarch64" => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    };

    /// <summary>
    /// Rust <c>TesterOsInfo::label()</c>: pretty distro + variant suffix + arch,
    /// e.g. <c>"Ubuntu 24.04 Server (x86_64)"</c>.
    /// </summary>
    public static string OsLabel(string distro, string version, string variant, string arch)
    {
        var prettyDistro = distro switch
        {
            "ubuntu" => "Ubuntu",
            "debian" => "Debian",
            "amazonlinux" or "amzn" => "Amazon Linux",
            "rhel" => "Red Hat",
            "centos" => "CentOS",
            _ => distro,
        };

        var variantSuffix = variant switch
        {
            "desktop" => " Desktop",
            "server" => " Server",
            _ => string.Empty,
        };

        return $"{prettyDistro} {version}{variantSuffix} ({arch})";
    }

    /// <summary>
    /// Rust <c>install_prereqs</c> package-manager selection: ubuntu/debian =&gt;
    /// apt; amzn/amazonlinux/rhel/centos/fedora =&gt; dnf; else apt.
    /// </summary>
    public static string PackageManager(string distro) => distro switch
    {
        "ubuntu" or "debian" => "apt",
        "amzn" or "amazonlinux" or "rhel" or "centos" or "fedora" => "dnf",
        _ => "apt",
    };

    /// <summary>Rust <c>detect_os</c> SSH command (verbatim).</summary>
    public const string DetectOsCommand =
        "cat /etc/os-release 2>/dev/null; echo '---'; uname -m; uname -r; " +
        "dpkg -l ubuntu-desktop 2>/dev/null | grep -q '^ii' && echo 'VARIANT=desktop' || echo 'VARIANT=server'";

    /// <summary>Rust apt prereq command (verbatim).</summary>
    public const string AptPrereqCommand =
        "export DEBIAN_FRONTEND=noninteractive; " +
        "for i in $(seq 1 12); do sudo fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break; " +
        "echo 'waiting for apt lock...'; sleep 5; done; " +
        "sudo apt-get install -y -qq curl tar ca-certificates < /dev/null";

    /// <summary>Rust dnf prereq command (verbatim).</summary>
    public const string DnfPrereqCommand = "sudo dnf install -y curl tar ca-certificates";

    /// <summary>Rust <c>verify_binaries</c> command (verbatim).</summary>
    public const string VerifyBinariesCommand =
        "test -x /usr/local/bin/networker-tester && test -x /usr/local/bin/networker-agent && " +
        "/usr/local/bin/networker-tester --version";

    /// <summary>Rust <c>verify_installed</c> command (verbatim).</summary>
    public const string VerifyInstalledCommand =
        "test -x /usr/local/bin/networker-tester && test -x /usr/local/bin/networker-agent";

    /// <summary>
    /// Rust <c>install_prereqs</c>: the command for the chosen package manager.
    /// </summary>
    public static string PrereqCommand(string distro) =>
        PackageManager(distro) == "dnf" ? DnfPrereqCommand : AptPrereqCommand;

    /// <summary>
    /// Rust <c>download_binary</c> command for one candidate tag (verbatim, with
    /// url substituted). <paramref name="binary"/> is <c>networker-tester</c> or
    /// <c>networker-agent</c>; <paramref name="target"/> the release target
    /// triple.
    /// </summary>
    public static string DownloadBinaryCommand(string binary, string tag, string target)
    {
        var url = $"https://github.com/{Repo}/releases/download/{tag}/{binary}-{target}.tar.gz";
        return
            $"set -e; curl -fsSL --retry 2 --retry-delay 2 --max-time 120 {url} -o /tmp/{binary}.tar.gz < /dev/null " +
            $"&& tar xzf /tmp/{binary}.tar.gz -C /tmp " +
            $"&& sudo install -m 0755 /tmp/{binary} /usr/local/bin/{binary} " +
            $"&& rm -f /tmp/{binary}.tar.gz /tmp/{binary}";
    }

    /// <summary>
    /// Rust predicate <c>safe(s)</c> for agent env values: every char ASCII
    /// alphanumeric or one of <c>-_.:/</c>.
    /// </summary>
    public static bool IsSafeAgentEnvValue(string s)
    {
        foreach (var c in s)
        {
            var ok = char.IsAsciiLetterOrDigit(c) || c is '-' or '_' or '.' or ':' or '/';
            if (!ok)
            {
                return false;
            }
        }

        return true;
    }

    /// <summary>
    /// Rust <c>install_systemd_service</c>: build the user-level systemd unit file
    /// contents. When both key + url are present, they must pass
    /// <see cref="IsSafeAgentEnvValue"/> (else <see cref="ArgumentException"/> with
    /// the Rust message) and are emitted as <c>Environment=</c> lines; otherwise
    /// no env lines are emitted.
    /// </summary>
    public static string BuildSystemdUnit(string? agentApiKey, string? agentDashboardUrl)
    {
        string envLines;
        if (!string.IsNullOrEmpty(agentApiKey) && !string.IsNullOrEmpty(agentDashboardUrl))
        {
            if (!IsSafeAgentEnvValue(agentApiKey) || !IsSafeAgentEnvValue(agentDashboardUrl))
            {
                throw new ArgumentException(
                    "agent_api_key or agent_dashboard_url contains unsafe characters");
            }

            envLines = $"Environment=AGENT_API_KEY={agentApiKey}\nEnvironment=AGENT_DASHBOARD_URL={agentDashboardUrl}\n";
        }
        else
        {
            envLines = string.Empty;
        }

        // Verbatim Rust unit template with {env_lines} substituted.
        return
            "[Unit]\n" +
            "Description=Networker Agent\n" +
            "After=network.target\n" +
            "\n" +
            "[Service]\n" +
            "Type=simple\n" +
            "ExecStart=/usr/local/bin/networker-agent\n" +
            "Restart=on-failure\n" +
            "RestartSec=5\n" +
            "Environment=RUST_LOG=info\n" +
            envLines +
            "\n" +
            "[Install]\n" +
            "WantedBy=default.target\n";
    }

    /// <summary>
    /// Rust <c>install_systemd_service</c>: the start commands, which differ by
    /// whether agent config is present (enable --now vs enable only).
    /// </summary>
    public static string SystemdStartCommands(string? agentApiKey, string? agentDashboardUrl)
    {
        var configured = !string.IsNullOrEmpty(agentApiKey) && !string.IsNullOrEmpty(agentDashboardUrl);
        return configured
            ? "sudo loginctl enable-linger $(whoami) 2>/dev/null || true; " +
              "systemctl --user daemon-reload && systemctl --user enable --now networker-agent.service 2>&1 | tail -5"
            : "systemctl --user daemon-reload && systemctl --user enable networker-agent.service 2>/dev/null || true";
    }

    /// <summary>
    /// Rust <c>install_systemd_service</c>: the full install command that writes
    /// the unit heredoc + runs the start commands (verbatim; note the leading
    /// whitespace before {start_cmds} in the Rust source is preserved).
    /// </summary>
    public static string SystemdInstallCommand(string? agentApiKey, string? agentDashboardUrl)
    {
        var service = BuildSystemdUnit(agentApiKey, agentDashboardUrl);
        var startCmds = SystemdStartCommands(agentApiKey, agentDashboardUrl);
        return
            "mkdir -p ~/.config/systemd/user && cat > ~/.config/systemd/user/networker-agent.service <<'EOF'\n" +
            service + "\n" +
            "EOF\n" +
            "         " + startCmds;
    }

    /// <summary>
    /// Rust <c>install_browser_harness_at_tag</c> command (verbatim, with tag
    /// substituted). Installs Chrome + Node 20 + downloads chrome-harness files.
    /// </summary>
    public static string BrowserHarnessCommand(string tag) =>
        "set -e; export DEBIAN_FRONTEND=noninteractive; " +
        "# Install Chrome command -v google-chrome >/dev/null 2>&1 || " +
        "(curl -fsSL https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb -o /tmp/chrome.deb < /dev/null " +
        "&& sudo apt-get install -y -qq /tmp/chrome.deb < /dev/null && rm -f /tmp/chrome.deb ); " +
        "# Install Node.js 20 from NodeSource (Ubuntu's default is 12-18) command -v node >/dev/null 2>&1 || " +
        "(curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - < /dev/null " +
        "&& sudo apt-get install -y -qq nodejs < /dev/null ); " +
        "# Download chrome-harness files from the release sudo mkdir -p /opt/bench/chrome-harness " +
        "&& sudo chown $(whoami):$(whoami) /opt/bench/chrome-harness " +
        $"&& curl -fsSL https://github.com/{Repo}/archive/refs/tags/{tag}.tar.gz -o /tmp/nwk.tar.gz < /dev/null " +
        "&& tar xzf /tmp/nwk.tar.gz -C /tmp " +
        "&& cp /tmp/networker-tester-*/benchmarks/chrome-harness/package.json /opt/bench/chrome-harness/ " +
        "&& cp /tmp/networker-tester-*/benchmarks/chrome-harness/runner.js /opt/bench/chrome-harness/ " +
        "&& cp /tmp/networker-tester-*/benchmarks/chrome-harness/test-page.html /opt/bench/chrome-harness/ " +
        "&& rm -rf /tmp/nwk.tar.gz /tmp/networker-tester-*/ " +
        "&& cd /opt/bench/chrome-harness && npm install --production < /dev/null";

    /// <summary>
    /// Rust <c>ssh_run</c> arg list: the ssh invocation arguments (before the
    /// command). Faithful to the Rust <c>-o</c> flags and <c>{user}@{ip}</c>.
    /// Provided so a later wiring pass builds the exact same process.
    /// </summary>
    public static IReadOnlyList<string> SshArgs(string user, string ip, string command) =>
    [
        "-o", "StrictHostKeyChecking=accept-new",
        "-o", "ConnectTimeout=10",
        "-o", "BatchMode=yes",
        $"{user}@{ip}",
        command,
    ];

    /// <summary>
    /// Side-effecting SSH execution is a host-side action the control plane
    /// cannot perform in CI (no outbound SSH, no key material). A later wiring
    /// pass supplies a real SSH transport; until then this stub only logs the
    /// command it WOULD run and returns empty stdout.
    /// </summary>
    // TODO(phase3): wire a real SSH transport (spawn `ssh` with SshArgs) here.
    public static Task<string> RunSshStub(string user, string ip, string command, ILogger? logger = null)
    {
        logger?.LogInformation(
            "TODO(phase3) SSH not executed in control plane; would run on {User}@{Ip}: {Command}",
            user, ip, command);
        return Task.FromResult(string.Empty);
    }
}
