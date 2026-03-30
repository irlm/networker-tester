use crate::provisioner::VmInfo;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

const SSH_CONNECT_TIMEOUT: &str = "10";
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(2);
const HEALTH_POLL_MAX_WAIT: Duration = Duration::from_secs(60);

/// Allowlist validation for language names to prevent shell injection (RR-001).
fn validate_language_name(lang: &str) -> Result<()> {
    if lang.is_empty()
        || !lang
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        bail!("invalid language name: {lang:?} — must be alphanumeric, dash, underscore, or dot");
    }
    Ok(())
}

/// Execute a command on the remote VM via SSH with a timeout (RR-005).
async fn ssh_exec(ip: &str, cmd: &str) -> Result<String> {
    let fut = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "BatchMode=yes",
            &format!("azureuser@{ip}"),
            cmd,
        ])
        .output();

    let output = tokio::time::timeout(SSH_COMMAND_TIMEOUT, fut)
        .await
        .context("SSH command timed out (5min limit)")?
        .context("failed to execute ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SSH command failed on {ip}: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Copy a local file to the remote VM via SCP.
async fn scp_to(ip: &str, local: &str, remote: &str) -> Result<()> {
    let output = tokio::process::Command::new("scp")
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            &format!("ConnectTimeout={SSH_CONNECT_TIMEOUT}"),
            "-o",
            "BatchMode=yes",
            local,
            &format!("azureuser@{ip}:{remote}"),
        ])
        .output()
        .await
        .context("failed to execute scp")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("SCP to {ip}:{remote} failed: {}", stderr.trim());
    }
    Ok(())
}

/// Deploy a reference API to the target VM.
///
/// Steps:
/// 1. Create /opt/bench/ directory on the VM
/// 2. Copy shared TLS cert + key
/// 3. Run the language-specific deploy.sh script
/// 4. Deploy metrics-agent binary and start it on :9100
/// 5. Wait for /health endpoint to respond (max 60s)
pub async fn deploy_api(vm: &VmInfo, language: &str, bench_dir: &Path) -> Result<()> {
    validate_language_name(language)?;
    tracing::info!("Deploying {} API to {} ({})", language, vm.name, vm.ip);

    // 1. Create target directory
    ssh_exec(
        &vm.ip,
        "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench",
    )
    .await
    .context("creating /opt/bench on VM")?;

    // 2. Copy shared TLS certs
    let shared_dir = bench_dir.join("shared");
    let cert_path = shared_dir.join("cert.pem");
    let key_path = shared_dir.join("key.pem");

    if cert_path.exists() && key_path.exists() {
        scp_to(&vm.ip, cert_path/*safe*/.to_str().unwrap_or_default(), "/opt/bench/cert.pem")
            .await
            .context("copying cert.pem")?;
        scp_to(&vm.ip, key_path/*safe*/.to_str().unwrap_or_default(), "/opt/bench/key.pem")
            .await
            .context("copying key.pem")?;
        tracing::debug!("TLS certs copied to VM");
    } else {
        tracing::warn!(
            "Shared certs not found at {}, generating on VM",
            shared_dir.display()
        );
        let gen_script = shared_dir.join("generate-cert.sh");
        if gen_script.exists() {
            scp_to(
                &vm.ip,
                gen_script/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/generate-cert.sh",
            )
            .await?;
            ssh_exec(&vm.ip, "bash /opt/bench/generate-cert.sh").await?;
        }
    }

    // 3. Deploy the language-specific server binary/source
    tracing::info!("Deploying {language} server to VM");
    let api_dir = bench_dir.join("reference-apis");
    match language {
        "rust" => {
            // Build the endpoint if needed, then copy binary
            let binary = bench_dir.join("../target/release/networker-endpoint");
            if !binary.exists() {
                tracing::info!("Building networker-endpoint...");
                let status = tokio::process::Command::new("cargo")
                    .args(["build", "--release", "-p", "networker-endpoint"])
                    .current_dir(bench_dir.join(".."))
                    .status()
                    .await?;
                if !status.success() {
                    bail!("cargo build networker-endpoint failed");
                }
            }
            scp_to(&vm.ip, binary/*safe*/.to_str().unwrap_or_default(), "/opt/bench/server")
                .await
                .context("copying Rust binary")?;
            ssh_exec(&vm.ip, "chmod +x /opt/bench/server").await?;
            ssh_exec(&vm.ip, "nohup /opt/bench/server --cert /opt/bench/cert.pem --key /opt/bench/key.pem --https-port 8443 > /opt/bench/server.log 2>&1 &").await?;
        }
        "go" => {
            let lang_dir = api_dir.join("go");
            // Build locally if binary doesn't exist
            let go_binary = lang_dir.join("server");
            if !go_binary.exists() {
                let build_sh = lang_dir.join("build.sh");
                if build_sh.exists() {
                    tracing::info!("Building Go binary...");
                    let status = tokio::process::Command::new("bash")
                        .arg(build_sh/*safe*/.to_str().unwrap_or_default())
                        .current_dir(&lang_dir)
                        .status()
                        .await?;
                    if !status.success() {
                        bail!("Go build.sh failed");
                    }
                }
            }
            scp_to(&vm.ip, go_binary/*safe*/.to_str().unwrap_or_default(), "/opt/bench/go-server")
                .await
                .context("copying Go binary")?;
            ssh_exec(
                &vm.ip,
                "chmod +x /opt/bench/go-server && \
                BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                nohup /opt/bench/go-server > /opt/bench/go-server.log 2>&1 &",
            )
            .await?;
        }
        "nodejs" => {
            let lang_dir = api_dir.join("nodejs");
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/nodejs").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("server.js")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/nodejs/server.js",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("package.json")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/nodejs/package.json",
            )
            .await?;
            // Install Node.js if needed, then start
            ssh_exec(&vm.ip,
                "command -v node >/dev/null 2>&1 || { curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - < /dev/null && sudo apt-get install -y nodejs < /dev/null; }; \
                 pkill -f 'node.*server\\.js' || true; sleep 1; \
                 cd /opt/bench/nodejs && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup node server.js > /var/log/bench-nodejs.log 2>&1 &"
            ).await?;
        }
        "python" => {
            let lang_dir = api_dir.join("python");
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/python").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("server.py")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/python/server.py",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("requirements.txt")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/python/requirements.txt",
            )
            .await?;
            ssh_exec(&vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq python3 python3-venv python3-pip < /dev/null; \
                 cd /opt/bench/python && python3 -m venv venv && venv/bin/pip install --quiet -r requirements.txt; \
                 pkill -f 'uvicorn server:app' || true; sleep 1; \
                 cd /opt/bench/python && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup venv/bin/uvicorn server:app --host 0.0.0.0 --port 8443 \
                     --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem \
                     > /var/log/python-bench.log 2>&1 &"
            ).await?;
        }
        "java" => {
            let lang_dir = api_dir.join("java");
            // Build locally if jar doesn't exist
            let jar = lang_dir.join("server.jar");
            if !jar.exists() {
                let build_sh = lang_dir.join("build.sh");
                if build_sh.exists() {
                    tracing::info!("Building Java server.jar...");
                    let status = tokio::process::Command::new("bash")
                        .arg(build_sh/*safe*/.to_str().unwrap_or_default())
                        .current_dir(&lang_dir)
                        .status()
                        .await?;
                    if !status.success() {
                        bail!("Java build.sh failed");
                    }
                }
            }
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/java").await?;
            scp_to(&vm.ip, jar/*safe*/.to_str().unwrap_or_default(), "/opt/bench/java/server.jar")
                .await
                .context("copying Java server.jar")?;
            ssh_exec(&vm.ip,
                "command -v java >/dev/null 2>&1 || { sudo apt-get update -qq && sudo apt-get install -y -qq openjdk-21-jdk-headless < /dev/null; }; \
                 pkill -f 'java.*server.jar' || true; sleep 1; \
                 cd /opt/bench/java && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup java -jar server.jar > /opt/bench/java/server.log 2>&1 &"
            ).await?;
        }
        lang if lang.starts_with("csharp") => {
            let lang_dir = api_dir.join(lang);
            let publish_dir = lang_dir.join("publish");
            if !publish_dir.exists() {
                let build_sh = lang_dir.join("build.sh");
                if build_sh.exists() {
                    tracing::info!("Building {lang}...");
                    let status = tokio::process::Command::new("bash")
                        .arg(build_sh/*safe*/.to_str().unwrap_or_default())
                        .current_dir(&lang_dir)
                        .status()
                        .await?;
                    if !status.success() {
                        bail!("{lang} build.sh failed");
                    }
                } else {
                    bail!("No publish/ dir and no build.sh for {lang}");
                }
            }
            let remote_dir = format!("/opt/bench/{lang}");
            ssh_exec(&vm.ip, &format!("mkdir -p {remote_dir}")).await?;
            // SCP the publish directory contents
            let publish_str = publish_dir/*safe*/.to_str().unwrap_or_default();
            // Copy all files from publish/ via tar pipe
            ssh_exec(&vm.ip, &format!("pkill -f '{lang}' || true")).await?;
            // Use scp with the main binary (AOT produces a single binary, non-AOT has a DLL)
            let binary_name = lang;
            let binary_path = publish_dir.join(binary_name);
            if binary_path.exists() {
                scp_to(
                    &vm.ip,
                    binary_path/*safe*/.to_str().unwrap_or_default(),
                    &format!("{remote_dir}/{binary_name}"),
                )
                .await?;
                ssh_exec(
                    &vm.ip,
                    &format!(
                        "chmod +x {remote_dir}/{binary_name} && \
                     BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                     nohup {remote_dir}/{binary_name} > /var/log/{lang}.log 2>&1 &"
                    ),
                )
                .await?;
            } else {
                // Non-AOT: copy all publish files, run with dotnet
                // Use tar over ssh to copy the directory
                let tar_cmd = format!(
                    "cd {} && tar cf - . | ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 -o BatchMode=yes azureuser@{} 'tar xf - -C {}'",
                    publish_str, vm.ip, remote_dir
                );
                let status = tokio::process::Command::new("bash")
                    .args(["-c", &tar_cmd])
                    .output()
                    .await
                    .context("tar pipe for csharp publish")?;
                if !status.status.success() {
                    bail!("Failed to copy {lang} publish directory");
                }
                // Find the DLL to run
                let dll_name = format!("{lang}.dll");
                ssh_exec(
                    &vm.ip,
                    &format!(
                    "command -v dotnet >/dev/null 2>&1 || {{ echo 'dotnet not found'; exit 1; }}; \
                     BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                     nohup dotnet {remote_dir}/{dll_name} > /var/log/{lang}.log 2>&1 &"
                ),
                )
                .await?;
            }
        }
        "cpp" => {
            let lang_dir = api_dir.join("cpp");
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/cpp").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("server.cpp")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/cpp/server.cpp",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("CMakeLists.txt")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/cpp/CMakeLists.txt",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("build.sh")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/cpp/build.sh",
            )
            .await?;
            ssh_exec(&vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq build-essential cmake libboost-system-dev libboost-dev libssl-dev < /dev/null; \
                 cd /opt/bench/cpp && bash build.sh; \
                 pkill -f '/opt/bench/cpp/build/server' || true; sleep 1; \
                 BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup /opt/bench/cpp/build/server > /opt/bench/cpp/server.log 2>&1 &"
            ).await?;
        }
        "ruby" => {
            let lang_dir = api_dir.join("ruby");
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/ruby").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("config.ru")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/ruby/config.ru",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("Gemfile")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/ruby/Gemfile",
            )
            .await?;
            scp_to(
                &vm.ip,
                lang_dir.join("puma.rb")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/ruby/puma.rb",
            )
            .await?;
            ssh_exec(&vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq ruby ruby-dev build-essential libssl-dev < /dev/null && \
                 sudo gem install bundler --no-document < /dev/null; \
                 cd /opt/bench/ruby && bundle install --quiet; \
                 pkill -f 'puma.*config.ru' || true; sleep 1; \
                 cd /opt/bench/ruby && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup bundle exec puma -C puma.rb config.ru > /var/log/ruby-bench.log 2>&1 &"
            ).await?;
        }
        "php" => {
            let lang_dir = api_dir.join("php");
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/php").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("server.php")/*safe*/.to_str().unwrap_or_default(),
                "/opt/bench/php/server.php",
            )
            .await?;
            ssh_exec(&vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq php-cli php-dev php-curl libssl-dev < /dev/null; \
                 php -m | grep -q swoole || { sudo pecl install swoole < /dev/null && echo 'extension=swoole.so' | sudo tee /etc/php/*/cli/conf.d/20-swoole.ini; }; \
                 pkill -f 'php.*server.php' || true; sleep 1; \
                 cd /opt/bench/php && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup php server.php > /var/log/php-bench.log 2>&1 &"
            ).await?;
        }
        "nginx" => {
            let lang_dir = api_dir.join("nginx");
            ssh_exec(
                &vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq nginx < /dev/null",
            )
            .await?;
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/download").await?;
            scp_to(
                &vm.ip,
                lang_dir.join("nginx.conf")/*safe*/.to_str().unwrap_or_default(),
                "/tmp/bench-nginx.conf",
            )
            .await?;
            ssh_exec(
                &vm.ip,
                "sudo cp /tmp/bench-nginx.conf /etc/nginx/nginx.conf",
            )
            .await?;
            if lang_dir.join("generate-download-files.sh").exists() {
                scp_to(
                    &vm.ip,
                    lang_dir
                        .join("generate-download-files.sh")
                        .to_str()
                        .unwrap(),
                    "/opt/bench/generate-download-files.sh",
                )
                .await?;
                ssh_exec(&vm.ip, "chmod +x /opt/bench/generate-download-files.sh && /opt/bench/generate-download-files.sh /opt/bench/download").await?;
            }
            if lang_dir.join("health.json").exists() {
                scp_to(
                    &vm.ip,
                    lang_dir.join("health.json")/*safe*/.to_str().unwrap_or_default(),
                    "/opt/bench/health.json",
                )
                .await?;
            }
            ssh_exec(&vm.ip,
                "sudo mkdir -p /tmp/nginx_uploads && sudo chown www-data:www-data /tmp/nginx_uploads; \
                 sudo nginx -t && sudo systemctl restart nginx"
            ).await?;
        }
        _ => {
            bail!("Unsupported language: {language}. No deploy handler defined.");
        }
    }

    // 4. Deploy metrics-agent (non-fatal — benchmarks can proceed without it)
    if let Err(e) = deploy_metrics_agent(vm, bench_dir).await {
        tracing::warn!("metrics-agent deployment failed (non-fatal): {e}");
    }

    // 5. Wait for /health endpoint
    wait_for_health(vm).await?;

    tracing::info!("{language} API deployed and healthy on {}", vm.name);
    Ok(())
}

/// Deploy and start the metrics-agent on the VM.
async fn deploy_metrics_agent(vm: &VmInfo, bench_dir: &Path) -> Result<()> {
    // Try to find a pre-built metrics-agent binary
    let agent_binary = bench_dir.join("metrics-agent/target/release/metrics-agent");
    if !agent_binary.exists() {
        tracing::warn!(
            "metrics-agent binary not found at {}, skipping deployment",
            agent_binary.display()
        );
        return Ok(());
    }

    tracing::info!("Deploying metrics-agent to {}", vm.name);
    scp_to(
        &vm.ip,
        agent_binary/*safe*/.to_str().unwrap_or_default(),
        "/opt/bench/metrics-agent",
    )
    .await
    .context("copying metrics-agent binary")?;

    ssh_exec(
        &vm.ip,
        "pkill -f /opt/bench/metrics-agent || true; \
         chmod +x /opt/bench/metrics-agent; \
         nohup /opt/bench/metrics-agent > /opt/bench/metrics-agent.log 2>&1 &",
    )
    .await
    .context("starting metrics-agent")?;

    // Brief wait for the agent to bind its port
    tokio::time::sleep(Duration::from_secs(1)).await;
    tracing::debug!("metrics-agent started on {}:9100", vm.ip);
    Ok(())
}

/// Poll the /health endpoint until it responds or timeout.
async fn wait_for_health(vm: &VmInfo) -> Result<()> {
    tracing::info!("Waiting for /health on {}:8443...", vm.ip);
    let deadline = tokio::time::Instant::now() + HEALTH_POLL_MAX_WAIT;

    loop {
        if tokio::time::Instant::now() > deadline {
            bail!(
                "Timed out waiting for /health on {}:8443 after {}s",
                vm.ip,
                HEALTH_POLL_MAX_WAIT.as_secs()
            );
        }

        let result = tokio::process::Command::new("curl")
            .args([
                "-sk",
                "--connect-timeout",
                "5",
                "--max-time",
                "10",
                &format!("https://{}:8443/health", vm.ip),
            ])
            .output()
            .await;

        if let Ok(output) = result {
            if output.status.success() {
                let body = String::from_utf8_lossy(&output.stdout);
                if body.contains("ok") || body.contains("healthy") {
                    tracing::info!("/health responded OK on {}", vm.ip);
                    return Ok(());
                }
            }
        }

        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

/// Validate that the deployed API is functioning correctly.
pub async fn validate_api(vm: &VmInfo) -> Result<()> {
    tracing::info!("Validating API on {} ({})", vm.name, vm.ip);

    // 1. GET /health -- verify status:ok
    let health_output = curl_get(vm, "/health").await.context("GET /health")?;
    let health: serde_json::Value =
        serde_json::from_str(&health_output).context("parsing /health response as JSON")?;
    if health.get("status").and_then(|v| v.as_str()) != Some("ok") {
        bail!(
            "/health did not return status:ok, got: {}",
            health_output.trim()
        );
    }
    tracing::debug!("/health OK");

    // 2. GET /download/1024 -- verify exactly 1024 bytes
    let download_output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-o",
            "/dev/null",
            "-w",
            "%{size_download}",
            &format!("https://{}:8443/download/1024", vm.ip),
        ])
        .output()
        .await
        .context("curl /download/1024")?;

    let size_str = String::from_utf8_lossy(&download_output.stdout);
    let size: u64 = size_str
        .trim()
        .parse()
        .with_context(|| format!("parsing download size: '{}'", size_str.trim()))?;
    if size != 1024 {
        bail!("/download/1024 returned {size} bytes, expected 1024");
    }
    tracing::debug!("/download/1024 OK (1024 bytes)");

    // 3. POST /upload with 1024 bytes -- verify bytes_received
    let upload_body_data = "X".repeat(1024);
    let upload_output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/octet-stream",
            "--data-binary",
            &upload_body_data,
            &format!("https://{}:8443/upload", vm.ip),
        ])
        .output()
        .await
        .context("curl POST /upload")?;

    let upload_body = String::from_utf8_lossy(&upload_output.stdout);
    let upload_json: serde_json::Value = serde_json::from_str(&upload_body)
        .with_context(|| format!("parsing /upload response: '{}'", upload_body.trim()))?;
    let received = upload_json
        .get("bytes_received")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if received != 1024 {
        bail!("/upload reported {received} bytes_received, expected 1024");
    }
    tracing::debug!("/upload OK (1024 bytes received)");

    tracing::info!("API validation passed on {}", vm.name);
    Ok(())
}

/// Helper: curl GET an endpoint and return the body as a string.
async fn curl_get(vm: &VmInfo, path: &str) -> Result<String> {
    let output = tokio::process::Command::new("curl")
        .args([
            "-sk",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            &format!("https://{}:8443{}", vm.ip, path),
        ])
        .output()
        .await
        .context("curl GET")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("curl GET {path} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Stop the running API server on the VM.
pub async fn stop_api(vm: &VmInfo) -> Result<()> {
    tracing::info!("Stopping API on {} ({})", vm.name, vm.ip);
    // Kill common server process names
    ssh_exec(
        &vm.ip,
        "pkill -f '/opt/bench/.*server' || true; \
         pkill -f 'networker-endpoint' || true; \
         pkill -f '/opt/bench/metrics-agent' || true; \
         pkill -f 'node.*server\\.js' || true; \
         pkill -f 'uvicorn' || true; \
         pkill -f 'java.*server.jar' || true; \
         pkill -f 'dotnet' || true; \
         pkill -f 'puma' || true; \
         pkill -f 'ruby' || true; \
         pkill -f 'php.*server.php' || true; \
         sudo systemctl stop nginx 2>/dev/null || true",
    )
    .await
    .context("stopping server processes")?;

    tracing::info!("API stopped on {}", vm.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provisioner::VmInfo;

    fn test_vm() -> VmInfo {
        VmInfo {
            name: "test-vm".into(),
            ip: "127.0.0.1".into(),
            cloud: "azure".into(),
            os: "ubuntu".into(),
            vm_size: "Standard_D2s_v3".into(),
            resource_group: "alethabench-rg".into(),
        }
    }

    #[test]
    fn test_deploy_script_path_resolution() {
        let bench_dir = Path::new("/tmp/bench");
        let go_path = bench_dir.join("reference-apis/go/deploy.sh");
        assert_eq!(
            go_path/*safe*/.to_str().unwrap_or_default(),
            "/tmp/bench/reference-apis/go/deploy.sh"
        );
        let rust_path = bench_dir.join("reference-apis/rust-deploy.sh");
        assert_eq!(
            rust_path/*safe*/.to_str().unwrap_or_default(),
            "/tmp/bench/reference-apis/rust-deploy.sh"
        );
    }

    #[test]
    fn test_curl_url_formatting() {
        let vm = test_vm();
        let url = format!("https://{}:8443/health", vm.ip);
        assert_eq!(url, "https://127.0.0.1:8443/health");
    }
}
