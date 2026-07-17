//! SSH exec helpers: shell quoting/validation and remote server/proxy
//! deploy, start, stop, and log collection on benchmark VMs.

use super::status::log_callback;
use crate::callback::CallbackClient;
use crate::provisioner::VmInfo;
use crate::ssh;
use anyhow::{Context, Result};

/// Start a pre-deployed language server on an existing VM.
pub(super) async fn start_existing_server(vm: &VmInfo, language: &str) -> Result<()> {
    validate_shell_safe(language, "language")?;

    // Kill anything on port 8443
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let start_cmd = match language {
        "rust" => "nohup /opt/bench/rust-server --https-port 8443 > /dev/null 2>&1 &",
        "go" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/go-server > /dev/null 2>&1 &",
        "cpp" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/cpp-build/server > /dev/null 2>&1 &",
        "nodejs" => "BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup node /opt/bench/nodejs-server.js > /dev/null 2>&1 &",
        "python" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup uvicorn server:app --host 0.0.0.0 --port 8443 --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem --log-level error > /dev/null 2>&1 &",
        "java" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup java Server > /dev/null 2>&1 &",
        "ruby" => "cd /opt/bench/ruby && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup bundle exec puma -C puma.rb > /dev/null 2>&1 &",
        "php" => "BENCH_CERT_DIR=/opt/bench nohup php /opt/bench/php/server.php > /dev/null 2>&1 &",
        "nginx" => "sudo systemctl restart nginx",
        _ if language.starts_with("csharp-") => {
            // Handled below with dynamic string
            ""
        }
        _ => anyhow::bail!("Unknown language: {language}"),
    };

    if language.starts_with("csharp-") {
        // Single-quote the config-derived value so the remote shell treats it
        // as inert data (quotes may appear mid-word in a path: /opt/bench/'x'/'x'
        // resolves to the same path as /opt/bench/x/x).
        let cmd = format!(
            "chmod +x /opt/bench/{lang}/{lang} 2>/dev/null; BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup /opt/bench/{lang}/{lang} > /dev/null 2>&1 &",
            lang = shell_quote(language)
        );
        ssh::ssh_exec(&vm.ip, &cmd).await?;
    } else {
        ssh::ssh_exec(&vm.ip, start_cmd).await?;
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Health check
    for i in 0..15 {
        if let Ok(out) = ssh::ssh_exec(
            &vm.ip,
            "curl -sk --max-time 2 https://localhost:8443/health 2>/dev/null",
        )
        .await
        {
            if out.contains("ok") || out.contains("status") {
                tracing::info!("{} server healthy on {}", language, vm.ip);
                return Ok(());
            }
        }
        if i < 14 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    anyhow::bail!("{} server failed health check after 15s", language)
}

/// Stop any running server on port 8443.
#[allow(dead_code)]
pub(super) async fn stop_existing_server(vm: &VmInfo) {
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
}

// Deploy a reverse proxy on a VM. Uses install.sh --benchmark-proxy-swap.
// Token generation moved to crate::token_manager

/// Quote a string so it is transported as a single inert word through a POSIX
/// shell. Wraps in single quotes and escapes embedded single quotes with the
/// standard `'\''` dance. Within single quotes the shell performs NO expansion
/// (no `$(...)`, backticks, `$VAR`, globbing, or word splitting), so any
/// hostile value arrives at the target program byte-for-byte as data.
///
/// This is the primary injection defense for remote command construction;
/// `validate_shell_safe` remains as defense-in-depth on top.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Validate a name is safe for shell interpolation (alphanumeric + dash/underscore/dot).
fn validate_shell_safe(name: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
        "{label} name {name:?} contains unsafe characters for shell interpolation"
    );
    Ok(())
}

pub(super) async fn deploy_proxy(vm: &VmInfo, proxy: &str) -> Result<()> {
    validate_shell_safe(proxy, "proxy")?;
    tracing::info!("Deploying proxy {} on {}", proxy, vm.ip);
    // Deploy proxy with install.sh. The --benchmark-proxy-swap health check
    // may timeout because the upstream server isn't running yet — that's expected.
    // We run it in a subshell with || true, then verify nginx config separately.
    //
    // The install step runs in a plain subshell (...) rather than the previous
    // `bash -c '...'` wrapper: the wrapper added a second quoting layer, which
    // made the interpolated proxy value impossible to quote safely. With a
    // single evaluation layer, shell_quote() transports it as inert data.
    let cmd = format!(
        "export DEBIAN_FRONTEND=noninteractive; (curl -fsSL https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh | sudo -E bash -s -- --benchmark-proxy-swap {} 2>&1; true) && sudo nginx -t 2>&1",
        shell_quote(proxy)
    );
    ssh::ssh_exec(&vm.ip, &cmd)
        .await
        .with_context(|| format!("Failed to deploy proxy {proxy} on {}", vm.ip))?;
    tracing::info!(
        "Proxy {} deployed successfully (health check deferred until backend starts)",
        proxy
    );
    // Health check is deferred — in application mode, the backend (language server)
    // starts AFTER the proxy. The proxy will respond once the backend is running.
    Ok(())
}

/// Stop the current proxy and flush connections (isolation protocol).
pub(super) async fn stop_proxy(vm: &VmInfo) {
    tracing::info!("Stopping proxy on {}", vm.ip);
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo systemctl stop nginx caddy traefik haproxy apache2 httpd 2>/dev/null; sudo fuser -k 8443/tcp 2>/dev/null; sleep 2",
    )
    .await;
}

/// Deploy a language server in application mode (localhost:8080, no TLS).
/// Token is already on the VM at /opt/bench/.api-token (deployed via SCP).
/// The language server reads the token from that file at startup.
pub(super) async fn deploy_app_language(vm: &VmInfo, language: &str, proxy: &str) -> Result<()> {
    validate_shell_safe(language, "language")?;
    validate_shell_safe(proxy, "proxy")?;
    tracing::info!("Deploying {} in application mode on {}", language, vm.ip);
    // Server reads BENCH_API_TOKEN from /opt/bench/.api-token at startup.
    // LOG_FORMAT=json enables structured JSON logging on the server process.
    let cmd = format!(
        "export DEBIAN_FRONTEND=noninteractive LOG_FORMAT=json LOG_SERVICE={} BENCH_API_TOKEN=$(cat /opt/bench/.api-token 2>/dev/null) && curl -fsSL https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh | sudo -E bash -s -- --benchmark-server {} --benchmark-proxy {} 2>&1",
        shell_quote(language),
        shell_quote(language),
        shell_quote(proxy)
    );
    ssh::ssh_exec(&vm.ip, &cmd).await.with_context(|| {
        format!(
            "Failed to deploy {language} in application mode on {}",
            vm.ip
        )
    })?;
    Ok(())
}

/// Stop the language server on port 8080.
pub(super) async fn stop_app_language(vm: &VmInfo) {
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8080 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}

/// Collect recent log output from the benchmark server on the VM.
/// Reads the last 100 lines from the server process output and forwards
/// them via the log callback so they appear in the dashboard live log.
pub(super) async fn collect_server_logs(
    vm: &VmInfo,
    language: &str,
    callback: &CallbackClient,
    testbed_id: &str,
) {
    // Read last 100 lines from systemd journal or log file
    let cmd = "journalctl -u bench-server --no-pager -n 100 --output=cat 2>/dev/null || \
               tail -100 /opt/bench/server.log 2>/dev/null || \
               echo '(no server logs found)'";
    match ssh::ssh_exec(&vm.ip, cmd).await {
        Ok(output) => {
            let lines: Vec<String> = output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| format!("[{language}] {l}"))
                .collect();
            if !lines.is_empty() {
                tracing::info!(
                    language,
                    line_count = lines.len(),
                    "Collected server logs from VM"
                );
                log_callback(callback, testbed_id, lines).await;
            }
        }
        Err(e) => {
            tracing::debug!("Failed to collect server logs for {language}: {e:#}");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_quote_plain_values_unchanged_semantics() {
        // Values that validate_shell_safe accepts today must transport
        // byte-identically (behavior-preserving change).
        for v in ["nginx", "csharp-aot", "h2", "reuse", "a1B2.c-d_e"] {
            assert_eq!(shell_quote(v), format!("'{v}'"));
        }
    }

    #[test]
    fn test_shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_quote("'"), r"''\'''");
        assert_eq!(shell_quote(""), "''");
    }

    /// Prove a hostile config value is transported INERT through a real shell:
    /// command substitution must not execute, and the value must arrive
    /// byte-for-byte as data — exactly the layer ssh_exec hands the payload to
    /// (sshd runs the remote command through the login shell).
    #[test]
    fn test_shell_quote_hostile_value_is_inert_through_shell() {
        let canary = std::env::temp_dir().join(format!("pwn-canary-{}", std::process::id()));
        let _ = std::fs::remove_file(&canary);

        let hostile = format!("$(touch {}); `id`; $HOME; a'b\"c", canary.display());
        let cmd = format!("printf %s {}", shell_quote(&hostile));
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .expect("failed to spawn sh");

        assert!(out.status.success(), "shell rejected the quoted payload");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            hostile,
            "hostile value must arrive byte-for-byte as data"
        );
        assert!(
            !canary.exists(),
            "command substitution executed — shell_quote failed to neutralize $()"
        );
        let _ = std::fs::remove_file(&canary);
    }

    /// The deploy_proxy command template must keep the quoted value at a single
    /// shell evaluation layer (the old `bash -c '...'` wrapper nested it inside
    /// an outer single-quoted string, where no quoting scheme is safe).
    #[test]
    fn test_shell_quote_survives_single_eval_layer_with_metachars() {
        let hostile = "x; rm -rf /tmp/nonexistent-dir-pwn; echo owned";
        let cmd = format!("printf %s {}", shell_quote(hostile));
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .expect("failed to spawn sh");
        assert_eq!(String::from_utf8_lossy(&out.stdout), hostile);
    }
}
