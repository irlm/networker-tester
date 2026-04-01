use crate::provisioner::VmInfo;
use crate::ssh::{scp_dir_to, scp_to, ssh_exec, validate_ip};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

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

// ssh_exec, scp_to, scp_dir_to, validate_ip imported from crate::ssh

/// The GitHub repo URL for cloning reference APIs.
const REPO_URL: &str = "https://github.com/irlm/networker-tester.git";
const REPO_BRANCH: &str = "main";
/// GitHub raw URL for the installer script.
const INSTALLER_URL: &str = "https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh";

/// Deploy a language server to a remote VM using ONE SSH call.
/// Downloads the install script via HTTP and runs it — no SCP, no multiple connections.
async fn deploy_remote(vm: &VmInfo, language: &str) -> Result<()> {
    tracing::info!("Remote deploy: {} to {} via installer", language, vm.ip);

    // Build the install command based on language
    let install_cmd = match language {
        "rust" => {
            // Download pre-built endpoint from GitHub Releases, or fall back to install.sh
            format!(
                "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 curl -sLo /tmp/endpoint.tar.gz \
                   https://github.com/irlm/networker-tester/releases/latest/download/networker-endpoint-x86_64-unknown-linux-musl.tar.gz && \
                 tar xzf /tmp/endpoint.tar.gz -C /opt/bench/ && \
                 mv /opt/bench/networker-endpoint /opt/bench/server 2>/dev/null; \
                 if [ ! -s /opt/bench/server ]; then \
                     echo 'Release download failed, building from source...'; \
                     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y < /dev/null && \
                     source $HOME/.cargo/env && \
                     sudo apt-get update -qq && sudo apt-get install -y -qq git < /dev/null && \
                     git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null && \
                     cd /tmp/nwk-repo && cargo build --release -p networker-endpoint && \
                     cp target/release/networker-endpoint /opt/bench/server; \
                 fi; \
                 chmod +x /opt/bench/server && \
                 nohup /opt/bench/server --https-port 8443 > /opt/bench/server.log 2>&1 &"
            )
        }
        "nginx" => {
            format!(
                "sudo apt-get update -qq && sudo apt-get install -y -qq nginx git < /dev/null; \
                 sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null; \
                 if [ -f /tmp/nwk-repo/benchmarks/reference-apis/nginx/nginx.conf ]; then \
                     sudo cp /tmp/nwk-repo/benchmarks/reference-apis/nginx/nginx.conf /etc/nginx/nginx.conf; \
                 fi; \
                 sudo mkdir -p /opt/bench/download /tmp/nginx_uploads; \
                 sudo chown www-data:www-data /tmp/nginx_uploads; \
                 if [ -f /tmp/nwk-repo/benchmarks/reference-apis/nginx/generate-download-files.sh ]; then \
                     bash /tmp/nwk-repo/benchmarks/reference-apis/nginx/generate-download-files.sh /opt/bench/download; \
                 fi; \
                 sudo nginx -t && sudo systemctl restart nginx"
            )
        }
        "go" => {
            format!(
                "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 command -v go >/dev/null 2>&1 || {{ sudo snap install go --classic < /dev/null; }}; \
                 sudo apt-get update -qq && sudo apt-get install -y -qq git < /dev/null; \
                 git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null; \
                 cd /tmp/nwk-repo/benchmarks/reference-apis/go && go build -o /opt/bench/go-server . 2>/dev/null; \
                 chmod +x /opt/bench/go-server; \
                 BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup /opt/bench/go-server > /opt/bench/go-server.log 2>&1 &"
            )
        }
        "nodejs" => {
            format!(
                "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 command -v node >/dev/null 2>&1 || {{ sudo apt-get update -qq && sudo apt-get install -y -qq nodejs npm < /dev/null; }}; \
                 sudo apt-get install -y -qq git < /dev/null; \
                 git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null; \
                 cd /tmp/nwk-repo/benchmarks/reference-apis/nodejs && npm install --quiet 2>/dev/null; \
                 BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup node server.js > /var/log/bench-nodejs.log 2>&1 &"
            )
        }
        "python" => {
            format!(
                "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 sudo apt-get update -qq && sudo apt-get install -y -qq python3 python3-venv python3-pip git < /dev/null; \
                 git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null; \
                 cd /tmp/nwk-repo/benchmarks/reference-apis/python && \
                 python3 -m venv /opt/bench/pyenv && /opt/bench/pyenv/bin/pip install --quiet -r requirements.txt; \
                 BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup /opt/bench/pyenv/bin/uvicorn server:app --host 0.0.0.0 --port 8443 \
                     --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem \
                     > /var/log/python-bench.log 2>&1 &"
            )
        }
        other => {
            // Generic: clone repo and run deploy.sh if it exists
            format!(
                "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench; \
                 openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
                 -out /opt/bench/cert.pem -days 365 -nodes -subj '/CN=bench' 2>/dev/null; \
                 sudo apt-get update -qq && sudo apt-get install -y -qq git < /dev/null; \
                 git clone --depth 1 {REPO_URL} /tmp/nwk-repo 2>/dev/null; \
                 cd /tmp/nwk-repo/benchmarks/reference-apis/{other} && \
                 if [ -f deploy.sh ]; then BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 bash deploy.sh; \
                 elif [ -f build.sh ]; then bash build.sh && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 bash start.sh; \
                 else echo 'No deploy.sh or build.sh found for {other}'; exit 1; fi"
            )
        }
    };

    // ONE SSH call does everything — setup, download, build, start
    for attempt in 1..=5 {
        match ssh_exec(&vm.ip, &install_cmd).await {
            Ok(_) => {
                tracing::info!("Remote deploy succeeded for {} on {}", language, vm.ip);
                // Wait for health check
                return wait_for_health(vm).await;
            }
            Err(e) => {
                if attempt == 5 {
                    return Err(e).context(format!("remote deploy of {language} failed after 5 attempts"));
                }
                tracing::warn!("Deploy attempt {attempt}/5 failed for {language}, retrying in 10s...");
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }
    }
    unreachable!()
}

/// Deploy a reference API to the target VM.
///
/// Steps:
/// 1. Create /opt/bench/ directory and generate TLS certs on the VM
/// 2. Clone the repo (if not already cloned) to get reference-api source
/// 3. Deploy the language-specific server
/// 4. Wait for /health endpoint to respond (max 60s)
///
/// Works in two modes:
/// - **Local mode**: If `bench_dir/reference-apis/` exists locally, SCP files to the VM.
/// - **Remote mode**: If local files don't exist (production dashboard), clone the repo
///   on the VM and build from source there. This is the default for auto-provisioned VMs.
pub async fn deploy_api(vm: &VmInfo, language: &str, bench_dir: &Path) -> Result<()> {
    validate_language_name(language)?;
    validate_ip(&vm.ip)?;
    tracing::info!("Deploying {} API to {} ({})", language, vm.name, vm.ip);

    // Determine deploy mode
    let api_dir = bench_dir.join("reference-apis");
    let use_remote = !api_dir.join(language).exists() && !api_dir.exists();

    if use_remote {
        // Remote mode: ONE SSH call downloads install.sh and runs it.
        // The install script handles everything via HTTP — no SCP needed.
        return deploy_remote(vm, language).await;
    }

    // Local mode: SCP files from local disk (dev workflow)
    ssh_exec(&vm.ip, "sudo mkdir -p /opt/bench && sudo chown azureuser:azureuser /opt/bench")
        .await.context("creating /opt/bench on VM")?;

    // 4. Deploy the language-specific server
    tracing::info!("Deploying {language} server to VM (remote={use_remote})");
    let remote_api_base = "/opt/bench/repo/benchmarks/reference-apis";
    match language {
        "rust" => {
            let local_endpoint = std::path::Path::new("/usr/local/bin/networker-endpoint");
            if use_remote && local_endpoint.exists() {
                // SCP binary + start server in a single follow-up SSH call
                tracing::info!("Deploying Rust endpoint to {}...", vm.ip);
                // SCP binary, then wait before the next SSH to avoid connection rate-limiting
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                scp_to(&vm.ip, "/usr/local/bin/networker-endpoint", "/opt/bench/server")
                    .await
                    .context("copying Rust endpoint binary to VM")?;
                // Wait before next SSH — Azure VMs throttle rapid SSH connections
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                ssh_exec(&vm.ip,
                    "chmod +x /opt/bench/server && \
                     pkill -f '/opt/bench/server' 2>/dev/null; sleep 1; \
                     nohup /opt/bench/server --https-port 8443 > /opt/bench/server.log 2>&1 &"
                ).await.context("starting Rust server on VM")?;
            } else if use_remote {
                // No local binary — build on the VM from cloned repo
                ssh_exec(&vm.ip,
                    "if [ ! -f /opt/bench/server ]; then \
                        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y < /dev/null && \
                        source $HOME/.cargo/env && \
                        cd /opt/bench/repo && cargo build --release -p networker-endpoint && \
                        cp target/release/networker-endpoint /opt/bench/server; \
                    fi && chmod +x /opt/bench/server && \
                    pkill -f /opt/bench/server 2>/dev/null; sleep 1; \
                    nohup /opt/bench/server --https-port 8443 > /opt/bench/server.log 2>&1 &"
                ).await.context("building+starting Rust endpoint on VM")?;
            } else {
                // Local mode: SCP + start
                let binary = bench_dir.join("../target/release/networker-endpoint");
                if !binary.exists() {
                    let status = tokio::process::Command::new("cargo")
                        .args(["build", "--release", "-p", "networker-endpoint"])
                        .current_dir(bench_dir.join(".."))
                        .status().await?;
                    if !status.success() { bail!("cargo build networker-endpoint failed"); }
                }
                scp_to(&vm.ip, binary.to_str().unwrap_or_default(), "/opt/bench/server").await?;
                ssh_exec(&vm.ip, "chmod +x /opt/bench/server && \
                    pkill -f /opt/bench/server 2>/dev/null; sleep 1; \
                    nohup /opt/bench/server --https-port 8443 > /opt/bench/server.log 2>&1 &").await?;
            }
        }
        "go" => {
            if use_remote {
                // Build Go server on the VM from cloned repo
                ssh_exec(&vm.ip, &format!(
                    "command -v go >/dev/null 2>&1 || {{ sudo apt-get update -qq && sudo snap install go --classic < /dev/null; }}; \
                    cd {remote_api_base}/go && go build -o /opt/bench/go-server . 2>/dev/null"
                )).await.context("building Go server on VM")?;
            } else {
                let lang_dir = api_dir.join("go");
                let go_binary = lang_dir.join("server");
                if !go_binary.exists() {
                    let build_sh = lang_dir.join("build.sh");
                    if build_sh.exists() {
                        let status = tokio::process::Command::new("bash")
                            .arg(build_sh.to_str().unwrap_or_default())
                            .current_dir(&lang_dir)
                            .status()
                            .await?;
                        if !status.success() {
                            bail!("Go build.sh failed");
                        }
                    }
                }
                scp_to(&vm.ip, go_binary.to_str().unwrap_or_default(), "/opt/bench/go-server")
                    .await
                    .context("copying Go binary")?;
            }
            ssh_exec(
                &vm.ip,
                "chmod +x /opt/bench/go-server; pkill -f '/opt/bench/go-server' || true; sleep 1; \
                BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                nohup /opt/bench/go-server > /opt/bench/go-server.log 2>&1 &",
            )
            .await?;
        }
        "nodejs" => {
            let src = if use_remote {
                format!("{remote_api_base}/nodejs")
            } else {
                let lang_dir = api_dir.join("nodejs");
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/nodejs").await?;
                scp_to(&vm.ip, lang_dir.join("server.js").to_str().unwrap_or_default(), "/opt/bench/nodejs/server.js").await?;
                scp_to(&vm.ip, lang_dir.join("package.json").to_str().unwrap_or_default(), "/opt/bench/nodejs/package.json").await?;
                "/opt/bench/nodejs".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "command -v node >/dev/null 2>&1 || {{ sudo apt-get update -qq && sudo apt-get install -y -qq nodejs npm < /dev/null; }}; \
                 pkill -f 'node.*server\\.js' || true; sleep 1; \
                 cd {src} && npm install --quiet 2>/dev/null; \
                 cd {src} && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup node server.js > /var/log/bench-nodejs.log 2>&1 &"
            )).await?;
        }
        "python" => {
            let src = if use_remote {
                format!("{remote_api_base}/python")
            } else {
                let lang_dir = api_dir.join("python");
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/python").await?;
                scp_to(&vm.ip, lang_dir.join("server.py").to_str().unwrap_or_default(), "/opt/bench/python/server.py").await?;
                scp_to(&vm.ip, lang_dir.join("requirements.txt").to_str().unwrap_or_default(), "/opt/bench/python/requirements.txt").await?;
                "/opt/bench/python".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "sudo apt-get update -qq && sudo apt-get install -y -qq python3 python3-venv python3-pip < /dev/null; \
                 cd {src} && python3 -m venv /opt/bench/python-venv && /opt/bench/python-venv/bin/pip install --quiet -r requirements.txt; \
                 pkill -f 'uvicorn server:app' || true; sleep 1; \
                 cd {src} && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup /opt/bench/python-venv/bin/uvicorn server:app --host 0.0.0.0 --port 8443 \
                     --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem \
                     > /var/log/python-bench.log 2>&1 &"
            )).await?;
        }
        "java" => {
            let src = if use_remote {
                // Build on VM from cloned repo
                ssh_exec(&vm.ip, &format!(
                    "command -v java >/dev/null 2>&1 || {{ sudo apt-get update -qq && sudo apt-get install -y -qq openjdk-21-jdk-headless maven < /dev/null; }}; \
                     cd {remote_api_base}/java && bash build.sh 2>/dev/null; \
                     mkdir -p /opt/bench/java && cp server.jar /opt/bench/java/ 2>/dev/null || true"
                )).await.context("building Java on VM")?;
                "/opt/bench/java".to_string()
            } else {
                let lang_dir = api_dir.join("java");
                let jar = lang_dir.join("server.jar");
                if !jar.exists() {
                    let build_sh = lang_dir.join("build.sh");
                    if build_sh.exists() {
                        let status = tokio::process::Command::new("bash")
                            .arg(build_sh.to_str().unwrap_or_default())
                            .current_dir(&lang_dir)
                            .status().await?;
                        if !status.success() { bail!("Java build.sh failed"); }
                    }
                }
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/java").await?;
                scp_to(&vm.ip, jar.to_str().unwrap_or_default(), "/opt/bench/java/server.jar").await?;
                "/opt/bench/java".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "command -v java >/dev/null 2>&1 || {{ sudo apt-get update -qq && sudo apt-get install -y -qq openjdk-21-jdk-headless < /dev/null; }}; \
                 pkill -f 'java.*server.jar' || true; sleep 1; \
                 cd {src} && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup java -jar server.jar > /opt/bench/java/server.log 2>&1 &"
            )).await?;
        }
        lang if lang.starts_with("csharp") => {
            let lang_dir = api_dir.join(lang);
            let publish_dir = lang_dir.join("publish");
            if !publish_dir.exists() && !use_remote {
                let build_sh = lang_dir.join("build.sh");
                if build_sh.exists() {
                    tracing::info!("Building {lang}...");
                    let status = tokio::process::Command::new("bash")
                        .arg(build_sh.to_str().unwrap_or_default())
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
            if use_remote {
                // C# requires .NET SDK — install and build on VM using Docker
                ssh_exec(&vm.ip, &format!(
                    "command -v docker >/dev/null 2>&1 || {{ curl -fsSL https://get.docker.com | sudo sh < /dev/null; }}; \
                     cd {remote_api_base}/{lang} && \
                     sudo docker build -t bench-{lang} . 2>/dev/null && \
                     sudo docker run -d --name bench-{lang} --rm -p 8443:8443 \
                         -v /opt/bench/cert.pem:/app/cert.pem -v /opt/bench/key.pem:/app/key.pem \
                         -e BENCH_CERT_DIR=/app -e BENCH_PORT=8443 bench-{lang} 2>/dev/null || \
                     echo 'Docker build/run failed for {lang} — skipping'"
                )).await.context(format!("deploying {lang} via Docker on VM"))?;
                // Skip the local SCP path below
                return wait_for_health(vm).await;
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
                // Non-AOT: copy all publish files via shared scp_dir_to (with timeout)
                scp_dir_to(&vm.ip, publish_str, &remote_dir)
                    .await
                    .context("copying csharp publish directory")?;
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
            let src = if use_remote { format!("{remote_api_base}/cpp") } else {
                let lang_dir = api_dir.join("cpp");
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/cpp").await?;
                scp_to(&vm.ip, lang_dir.join("server.cpp").to_str().unwrap_or_default(), "/opt/bench/cpp/server.cpp").await?;
                scp_to(&vm.ip, lang_dir.join("CMakeLists.txt").to_str().unwrap_or_default(), "/opt/bench/cpp/CMakeLists.txt").await?;
                scp_to(&vm.ip, lang_dir.join("build.sh").to_str().unwrap_or_default(), "/opt/bench/cpp/build.sh").await?;
                "/opt/bench/cpp".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "sudo apt-get update -qq && sudo apt-get install -y -qq build-essential cmake libboost-system-dev libboost-dev libssl-dev < /dev/null; \
                 cd {src} && bash build.sh; \
                 pkill -f 'cpp.*server\\|/opt/bench/cpp' || true; sleep 1; \
                 BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup {src}/build/server > /opt/bench/cpp-server.log 2>&1 &"
            )).await?;
        }
        "ruby" => {
            let src = if use_remote { format!("{remote_api_base}/ruby") } else {
                let lang_dir = api_dir.join("ruby");
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/ruby").await?;
                scp_to(&vm.ip, lang_dir.join("config.ru").to_str().unwrap_or_default(), "/opt/bench/ruby/config.ru").await?;
                scp_to(&vm.ip, lang_dir.join("Gemfile").to_str().unwrap_or_default(), "/opt/bench/ruby/Gemfile").await?;
                scp_to(&vm.ip, lang_dir.join("puma.rb").to_str().unwrap_or_default(), "/opt/bench/ruby/puma.rb").await?;
                "/opt/bench/ruby".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "sudo apt-get update -qq && sudo apt-get install -y -qq ruby ruby-dev build-essential libssl-dev < /dev/null && \
                 sudo gem install bundler --no-document < /dev/null; \
                 cd {src} && bundle install --quiet; \
                 pkill -f 'puma.*config.ru' || true; sleep 1; \
                 cd {src} && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup bundle exec puma -C puma.rb config.ru > /var/log/ruby-bench.log 2>&1 &"
            )).await?;
        }
        "php" => {
            let src = if use_remote { format!("{remote_api_base}/php") } else {
                let lang_dir = api_dir.join("php");
                ssh_exec(&vm.ip, "mkdir -p /opt/bench/php").await?;
                scp_to(&vm.ip, lang_dir.join("server.php").to_str().unwrap_or_default(), "/opt/bench/php/server.php").await?;
                "/opt/bench/php".to_string()
            };
            ssh_exec(&vm.ip, &format!(
                "sudo apt-get update -qq && sudo apt-get install -y -qq php-cli php-dev php-curl libssl-dev < /dev/null; \
                 php -m | grep -q swoole || {{ sudo pecl install swoole < /dev/null && echo 'extension=swoole.so' | sudo tee /etc/php/*/cli/conf.d/20-swoole.ini; }}; \
                 pkill -f 'php.*server.php' || true; sleep 1; \
                 cd {src} && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 \
                 nohup php server.php > /var/log/php-bench.log 2>&1 &"
            )).await?;
        }
        "nginx" => {
            ssh_exec(&vm.ip,
                "sudo apt-get update -qq && sudo apt-get install -y -qq nginx < /dev/null",
            ).await?;
            ssh_exec(&vm.ip, "mkdir -p /opt/bench/download").await?;

            if use_remote {
                // Use nginx.conf from cloned repo
                ssh_exec(&vm.ip, &format!(
                    "sudo cp {remote_api_base}/nginx/nginx.conf /etc/nginx/nginx.conf; \
                     if [ -f {remote_api_base}/nginx/generate-download-files.sh ]; then \
                         bash {remote_api_base}/nginx/generate-download-files.sh /opt/bench/download; \
                     fi; \
                     if [ -f {remote_api_base}/nginx/health.json ]; then \
                         cp {remote_api_base}/nginx/health.json /opt/bench/health.json; \
                     fi"
                )).await?;
            } else {
                let lang_dir = api_dir.join("nginx");
                scp_to(&vm.ip, lang_dir.join("nginx.conf").to_str().unwrap_or_default(), "/tmp/bench-nginx.conf").await?;
                ssh_exec(&vm.ip, "sudo cp /tmp/bench-nginx.conf /etc/nginx/nginx.conf").await?;
                if lang_dir.join("generate-download-files.sh").exists() {
                    scp_to(&vm.ip, lang_dir.join("generate-download-files.sh").to_str().unwrap_or_default(), "/opt/bench/generate-download-files.sh").await?;
                    ssh_exec(&vm.ip, "chmod +x /opt/bench/generate-download-files.sh && /opt/bench/generate-download-files.sh /opt/bench/download").await?;
                }
                if lang_dir.join("health.json").exists() {
                    scp_to(&vm.ip, lang_dir.join("health.json").to_str().unwrap_or_default(), "/opt/bench/health.json").await?;
                }
            }
            ssh_exec(&vm.ip,
                "sudo mkdir -p /tmp/nginx_uploads && sudo chown www-data:www-data /tmp/nginx_uploads; \
                 sudo nginx -t && sudo systemctl restart nginx"
            ).await?;
        }
        _ => {
            let deploy_script = bench_dir.join(format!("reference-apis/{language}/deploy.sh"));
            if deploy_script.exists() {
                scp_to(
                    &vm.ip,
                    deploy_script/*safe*/.to_str().unwrap_or_default(),
                    "/opt/bench/deploy.sh",
                )
                .await?;
                ssh_exec(
                    &vm.ip,
                    "chmod +x /opt/bench/deploy.sh && bash /opt/bench/deploy.sh",
                )
                .await
                .with_context(|| format!("deploy.sh for {language}"))?;
            } else {
                bail!("Unsupported language: {language}. No deploy handler defined.");
            }
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
            region: "eastus".into(),
            os: "ubuntu".into(),
            vm_size: "Standard_D2s_v3".into(),
            resource_group: "alethabench-rg".into(),
            ssh_user: "azureuser".into(),
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
