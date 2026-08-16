#Requires -Version 5.1
<#
.SYNOPSIS
  networker native Windows lab -- the whole managed path as PROCESSES on one
  Windows machine (GitHub windows-latest in CI, or your own dev box).

.DESCRIPTION
  The Docker lab (lab/lab.sh) proves the managed path on Linux containers. This
  is its native-Windows twin: everything is built from THIS checkout and runs as
  plain Windows processes, so we confirm that a WINDOWS TARGET
  (networker-endpoint.exe bare on :8443 and behind IIS on :8082/:8445 -- the
  same IIS setup every cloud Windows endpoint VM gets) offers and passes the
  same probe matrix as the Linux targets, and that a WINDOWS RUNNER
  (Networker.Agent + networker-tester.exe) executes it.

    PostgreSQL  (existing server via LAB_PG_*, or docker-compose.dashboard.yml postgres)
    control plane   dotnet Networker.ControlPlane.dll   http://127.0.0.1:5030
    target-1 rust   C:\networker\networker-endpoint.exe   8080/8443 + UDP 9997-9999
    target-2 iis    IIS site "networker-iis" 8082/8445 (ARR -> endpoint) [-NoIis to skip]
    runner-1        networker-agent.exe (AGENT_API_KEY, WS to the control plane)

  Then `validate` drives lab/validate.sh -- the SAME assertion matrix the
  Docker lab and the prod canary use -- through Git Bash.

    .\lab\native\lab-native.ps1 build            # cargo (Windows CI feature set) + dotnet publish
    .\lab\native\lab-native.ps1 up               # start + register + wait until the runner is online
    .\lab\native\lab-native.ps1 validate         # end-to-end matrix; non-zero on any regression
    .\lab\native\lab-native.ps1 status | logs | env | down

  Run from an ELEVATED PowerShell (IIS setup, C:\networker, firewall rules).
  Windows PowerShell 5.1 and pwsh 7 both work.

.PARAMETER Command
  build | up | validate | status | logs | env | down | all (build+up+validate)
.PARAMETER NoIis
  Skip the IIS target (targets = rust only; phase 2 of validate is then skipped).
.PARAMETER IisSetup
  cloud (default) -- the IIS payload install.sh generates for every cloud
  Windows endpoint (_iis_setup_powershell: IIS + URL Rewrite + ARR, HTTP 8082
  + HTTPS 8445 bindings, web.config proxy rules, HTTP/3 registry). Needs Git
  Bash to render it (bash -c 'source install.sh; _iis_setup_powershell').
  installer -- `install.ps1 -Setup iis` as shipped today: an HTTP-only stub
  (site on 8082, no 8445/ARR/H3) -- the proxy matrix on 8445 CANNOT pass with
  it; kept selectable so that gap stays visible.
.PARAMETER Fqdn
  Hostname to bind the IIS HTTPS/SNI listener to AND to register the targets
  under (default: 127.0.0.1). http.sys only speaks HTTP/3 on SNI hostname
  bindings, so with the default IP literal the IIS h3 modes are excluded from
  the matrix (LAB_H3_OFF_STACKS=iis) -- honestly, not silently.
.PARAMETER DryRun
  Print every external action instead of performing it (used to exercise the
  orchestration logic under pwsh on Linux/macOS in CI-less environments).
.PARAMETER DebugBuild
  Build the Rust binaries without --release (faster cold build).
.PARAMETER ValidateArgs
  Extra arguments passed to lab/validate.sh -- just append them:
  lab-native.ps1 validate --only 1 --runs 1   /   validate --modes tcp,http1
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet('build', 'up', 'validate', 'status', 'logs', 'env', 'down', 'all', 'help')]
    [string]$Command = 'help',
    [switch]$NoIis,
    [ValidateSet('cloud', 'installer')]
    [string]$IisSetup = 'cloud',
    [string]$Fqdn = '',
    [switch]$DebugBuild,
    [switch]$DryRun,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ValidateArgs = @()
)

Set-StrictMode -Version 1.0
# 'Continue' on purpose: Windows PowerShell 5.1 turns redirected native stderr
# (2>&1, 2>$null) into terminating NativeCommandError under 'Stop'. Cmdlets that
# must fail loudly pass -ErrorAction Stop explicitly; native tools are checked
# through $LASTEXITCODE.
$ErrorActionPreference = 'Continue'
$script:ProjectId = ''

# -- Paths ---------------------------------------------------------------------
$NativeDir = $PSScriptRoot
$LabDir    = Split-Path -Parent $NativeDir
$RepoRoot  = Split-Path -Parent $LabDir
$StateDir  = Join-Path $NativeDir '.state'
$BinDir    = Join-Path $StateDir 'bin'
$LogDir    = Join-Path $StateDir 'logs'
$PubDir    = Join-Path $StateDir 'publish'
$PidsFile  = Join-Path $StateDir 'pids.json'
$StateEnv  = Join-Path $StateDir 'lab.env'      # what lab/validate.sh sources (LAB_STATE_ENV)
$KeyFile   = Join-Path $StateDir 'runner-1.key'
$IsWin     = ($env:OS -eq 'Windows_NT')
$ExeSuffix = if ($IsWin) { '.exe' } else { '' }
# The IIS payload (install.sh _iis_setup_powershell) hard-codes this path for
# generate-site; the installer-tests Windows job uses the same location.
$EndpointInstallDir = 'C:\networker'

# -- Tunables (env-overridable; same defaults / semantics as lab/lab.sh) -------
function EnvOr($name, $default) { $v = [Environment]::GetEnvironmentVariable($name); if ([string]::IsNullOrEmpty($v)) { $default } else { $v } }
$CpPort        = [int](EnvOr 'LAB_CP_PORT' '5030')
$BaseUrl       = "http://127.0.0.1:$CpPort"
$AdminEmail    = EnvOr 'LAB_ADMIN_EMAIL' 'admin@lab.local'
$AdminBootPw   = EnvOr 'LAB_ADMIN_BOOTSTRAP_PASSWORD' 'LabBootstrap-Pass1!'
$AdminPw       = EnvOr 'LAB_ADMIN_PASSWORD' 'LabAdmin-Pass1!'
$ProjectName   = EnvOr 'LAB_PROJECT_NAME' 'Local Lab (native windows)'
$JwtSecret     = EnvOr 'LAB_JWT_SECRET' 'nwk-lab-jwt-secret-0123456789abcdef0123456789abcdef'
$CredKey       = EnvOr 'LAB_CREDENTIAL_KEY' '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
$StartupTimeout = [int](EnvOr 'LAB_STARTUP_TIMEOUT' '240')
$AgentTimeout   = [int](EnvOr 'LAB_AGENT_TIMEOUT' '180')
$IisTimeout     = [int](EnvOr 'LAB_IIS_TIMEOUT' '180')
# Postgres: LAB_PG_MODE = auto | psql | docker
#   psql   -> a server is already listening at LAB_PG_HOST:LAB_PG_PORT (CI:
#             ikalnytskyi/action-setup-postgres) and `psql` is on PATH
#   docker -> docker compose -f docker-compose.dashboard.yml up -d postgres
#             (DEV_PG_PORT), psql via `docker compose exec` when not on PATH
$PgMode  = EnvOr 'LAB_PG_MODE' 'auto'
$PgHost  = EnvOr 'LAB_PG_HOST' '127.0.0.1'
$PgUser  = EnvOr 'LAB_PG_USER' 'networker'
$PgPass  = EnvOr 'LAB_PG_PASSWORD' 'networker'
$PgDb    = EnvOr 'LAB_PG_DB' 'networker_core'
$PgPort  = 0   # resolved in Resolve-Postgres (LAB_PG_PORT > .dev.env DEV_PG_PORT > 5432)
$TargetHost = if ($Fqdn) { $Fqdn } else { EnvOr 'LAB_TARGET_HOST' '127.0.0.1' }
$Targets    = if ($NoIis) { 'rust' } else { 'rust,iis' }
$RunnerName = 'runner-1'

# -- Output helpers ------------------------------------------------------------
function Note($m) { Write-Host "> $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "  [ok]   $m" -ForegroundColor Green }
function Warn($m) { Write-Host "  [warn] $m" -ForegroundColor Yellow }
function Dim($m)  { Write-Host "         $m" -ForegroundColor DarkGray }
function Die($m)  { Write-Host "  [x]    $m" -ForegroundColor Red; exit 1 }
function Have($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function Ensure-Dir($p) { if (-not (Test-Path $p)) { New-Item -ItemType Directory -Force -Path $p | Out-Null } }

# -- Process helpers -----------------------------------------------------------
# Run an external command synchronously, streaming output; throw on non-zero.
function Run($exe, [string[]]$argv, $cwd = $RepoRoot) {
    $shown = "$exe $($argv -join ' ')"
    if ($DryRun) { Dim "[dry-run] $shown  (cwd $cwd)"; return }
    Dim $shown
    Push-Location $cwd
    try {
        & $exe @argv
        if ($LASTEXITCODE -ne 0) { throw "command failed ($LASTEXITCODE): $shown" }
    } finally { Pop-Location }
}
# Run and capture stdout as one string (stderr merged), never throws.
function Capture($exe, [string[]]$argv, $cwd = $RepoRoot) {
    if ($DryRun) { Dim "[dry-run] $exe $($argv -join ' ')"; return '' }
    Push-Location $cwd
    try { $out = & $exe @argv 2>&1 | Out-String; return $out } catch { return "$_" } finally { Pop-Location }
}
# Start a long-lived background process (hidden window, stdout/stderr to logs)
# with extra environment variables; records the pid under $name.
function Start-Managed($name, $exe, [string[]]$argv, [hashtable]$envVars, $cwd = $RepoRoot) {
    Ensure-Dir $LogDir
    $out = Join-Path $LogDir "$name.log"
    $err = Join-Path $LogDir "$name.err.log"
    if ($DryRun) {
        Dim "[dry-run] start $name : $exe $($argv -join ' ')"
        foreach ($k in $envVars.Keys) { $shown = if ($k -match 'KEY|SECRET|PASSWORD') { '***' } else { $envVars[$k] }; Dim "           $k=$shown" }
        return 0
    }
    $saved = @{}
    foreach ($k in $envVars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, [string]$envVars[$k]) }
    try {
        # -ArgumentList rejects an empty array -> splat it only when non-empty.
        $sp = @{ FilePath = $exe; WorkingDirectory = $cwd; WindowStyle = 'Hidden'; RedirectStandardOutput = $out; RedirectStandardError = $err; PassThru = $true }
        if ($argv -and $argv.Count -gt 0) { $sp['ArgumentList'] = $argv }
        $p = Start-Process @sp
    } finally {
        foreach ($k in $envVars.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
    $pids = Load-Pids
    $pids[$name] = $p.Id
    Save-Pids $pids
    Ok "$name started (pid $($p.Id)) -> $out"
    return $p.Id
}
function Load-Pids { if (Test-Path $PidsFile) { $h = @{}; $o = Get-Content $PidsFile -Raw | ConvertFrom-Json; foreach ($pp in $o.PSObject.Properties) { $h[$pp.Name] = [int]$pp.Value }; return $h } else { return @{} } }
function Save-Pids($h) { Ensure-Dir $StateDir; ($h | ConvertTo-Json) | Set-Content -Path $PidsFile -Encoding ascii }
function Stop-Managed($name) {
    $pids = Load-Pids
    if (-not $pids.ContainsKey($name)) { return }
    $procId = $pids[$name]
    if ($DryRun) { Dim "[dry-run] stop $name (pid $procId)" } else {
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($p) { Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue; Ok "$name stopped (pid $procId)" } else { Dim "$name (pid $procId) already gone" }
    }
    $pids.Remove($name); Save-Pids $pids
}
function Wait-Http($url, $timeoutSec, $label) {
    if ($DryRun) { Dim "[dry-run] wait for $url (<= ${timeoutSec}s)"; return $true }
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            if ($url -like 'https://*') {
                # PS 5.1 has no -SkipCertificateCheck; curl.exe is in-box on Windows 10 1803+ / Server 2019+.
                & curl.exe -sk --max-time 5 -o NUL $url 2>$null
                if ($LASTEXITCODE -eq 0) { return $true }
            } else {
                Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 5 -ErrorAction Stop | Out-Null
                return $true
            }
        } catch { }
        Start-Sleep -Seconds 2
    }
    Warn "$label not ready after ${timeoutSec}s ($url)"
    return $false
}
function Tcp-Open($h, [int]$port) {
    try { $c = New-Object Net.Sockets.TcpClient; $iar = $c.BeginConnect($h, $port, $null, $null); $okc = $iar.AsyncWaitHandle.WaitOne(1500); if ($okc) { $c.EndConnect($iar) }; $c.Close(); return $okc } catch { return $false }
}

# -- Toolchain discovery -------------------------------------------------------
function Find-Bash {
    # Git Bash (NOT the WSL launcher in System32): validate.sh + the IIS payload
    # generator source install.sh, which needs a real bash + coreutils + jq.
    if ($env:LAB_BASH -and (Test-Path $env:LAB_BASH)) { return $env:LAB_BASH }
    if (-not $IsWin) { $b = Get-Command bash -ErrorAction SilentlyContinue; if ($b) { return $b.Source }; return $null }
    $cands = @()
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) { $gitRoot = Split-Path (Split-Path $git.Source); $cands += (Join-Path $gitRoot 'bin\bash.exe'); $cands += (Join-Path (Split-Path $gitRoot) 'bin\bash.exe') }
    $cands += 'C:\Program Files\Git\bin\bash.exe'
    $cands += 'C:\Program Files (x86)\Git\bin\bash.exe'
    foreach ($c in $cands) { if ($c -and (Test-Path $c)) { return $c } }
    return $null
}
function To-BashPath($p) { return ($p -replace '\\', '/') }
function Find-WindowsPowerShell {
    # The IIS payload uses ServerManager (Install-WindowsFeature) + WebAdministration;
    # both are native to Windows PowerShell 5.1 (pwsh 7 loads them via WinCompat,
    # which is flaky for Install-WindowsFeature) -- so run it under powershell.exe.
    if (-not $env:SystemRoot) { return $null }
    $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (Test-Path $ps) { return $ps }
    return $null
}

# -- HTTP / API helpers (control plane) ----------------------------------------
$script:Token = ''
function Api($method, $path, $body = $null) {
    $headers = @{ Authorization = "Bearer $script:Token" }
    $uri = "$BaseUrl$path"
    if ($null -ne $body) {
        $json = if ($body -is [string]) { $body } else { $body | ConvertTo-Json -Compress -Depth 10 }
        return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers -ContentType 'application/json' -Body $json -TimeoutSec 60 -ErrorAction Stop
    }
    return Invoke-RestMethod -Method $method -Uri $uri -Headers $headers -TimeoutSec 60 -ErrorAction Stop
}
function Try-Login($email, $password) {
    try {
        $r = Invoke-RestMethod -Method Post -Uri "$BaseUrl/api/auth/login" -ContentType 'application/json' `
            -Body (@{ email = $email; password = $password } | ConvertTo-Json -Compress) -TimeoutSec 15 -ErrorAction Stop
        if ($r -and $r.token) { return [string]$r.token }
    } catch { }
    return ''
}
function Authenticate {
    # Mirror of lab.sh authenticate(): first boot must change the bootstrap password.
    if ($DryRun) { $script:Token = 'dry-run-token'; Dim '[dry-run] login + change-password'; return }
    $t = Try-Login $AdminEmail $AdminPw
    if (-not $t) {
        $t = Try-Login $AdminEmail $AdminBootPw
        if (-not $t) { Die "login failed for $AdminEmail (bootstrap and final passwords) -- is the control plane up? ($BaseUrl)" }
        $resp = Invoke-RestMethod -Method Post -Uri "$BaseUrl/api/auth/change-password" -Headers @{ Authorization = "Bearer $t" } `
            -ContentType 'application/json' -Body (@{ current_password = $AdminBootPw; new_password = $AdminPw } | ConvertTo-Json -Compress) -TimeoutSec 15 -ErrorAction Stop
        if (-not $resp.success) { Die "change-password failed: $($resp | ConvertTo-Json -Compress)" }
        $t = Try-Login $AdminEmail $AdminPw
        if (-not $t) { Die 'login failed after password change' }
        Ok "admin password set ($AdminEmail)"
    }
    $script:Token = $t
    # Prove the token is usable (status cache tolerance, see lab.sh).
    for ($i = 0; $i -lt 15; $i++) {
        try { Api GET '/api/projects' | Out-Null; return } catch { Start-Sleep -Seconds 1 }
    }
    Die 'authenticated API call keeps failing'
}
function Ensure-Project {
    if ($DryRun) { $script:ProjectId = '00000000-0000-0000-0000-000000000000'; Dim "[dry-run] ensure project '$ProjectName'"; return }
    $list = Api GET '/api/projects'
    $items = if ($list -is [array]) { $list } elseif ($list.PSObject.Properties['projects']) { $list.projects } else { @($list) }
    $existing = @($items | Where-Object { $_.name -eq $ProjectName })
    if ($existing.Count -gt 0) { $script:ProjectId = [string]$existing[0].project_id; Ok "project '$ProjectName' ($script:ProjectId)"; return }
    $created = Api POST '/api/projects' @{ name = $ProjectName; description = 'native windows lab -- runner/targets are local processes' }
    if (-not $created.project_id) { Die "project create failed: $($created | ConvertTo-Json -Compress)" }
    $script:ProjectId = [string]$created.project_id
    Ok "project '$ProjectName' created ($script:ProjectId)"
}
function Online-Runners {
    $agents = Api GET "/api/projects/$script:ProjectId/agents"
    $items = if ($agents -is [array]) { $agents } elseif ($agents.PSObject.Properties['agents']) { $agents.agents } elseif ($agents.PSObject.Properties['items']) { $agents.items } else { @() }
    return @($items | Where-Object { $_.status -eq 'online' -and $_.name -like 'runner-*' })
}

# -- Postgres ------------------------------------------------------------------
$script:PsqlVia = ''   # 'psql' | 'docker'
function Read-DevEnvPort {
    $f = Join-Path $RepoRoot '.dev.env'
    if (Test-Path $f) { foreach ($line in Get-Content $f) { if ($line -match '^DEV_PG_PORT=(\d+)') { return [int]$Matches[1] } } }
    return 0
}
function Resolve-Postgres {
    $explicit = EnvOr 'LAB_PG_PORT' ''
    $script:PgPort = if ($explicit) { [int]$explicit } else { $d = Read-DevEnvPort; if ($d -gt 0) { $d } else { 5432 } }
    $mode = $PgMode
    if ($mode -eq 'auto') {
        if ((Tcp-Open $PgHost $script:PgPort) -and (Have 'psql')) { $mode = 'psql' }
        elseif (Have 'docker') { $mode = 'docker' }
        elseif (Tcp-Open $PgHost $script:PgPort) { $mode = 'psql' }
        else { Die "no PostgreSQL: nothing listens on ${PgHost}:$script:PgPort and docker is not available (set LAB_PG_HOST/PORT/USER/PASSWORD/DB for an existing server, or install Docker Desktop)" }
    }
    if ($mode -eq 'docker') {
        Note "starting PostgreSQL via docker-compose.dashboard.yml (host port $script:PgPort)"
        $env:DEV_PG_PORT = "$script:PgPort"
        Run 'docker' @('compose', '-f', (Join-Path $RepoRoot 'docker-compose.dashboard.yml'), 'up', '-d', 'postgres')
        if (-not $DryRun) {
            $deadline = (Get-Date).AddSeconds(90)
            while (-not (Tcp-Open $PgHost $script:PgPort) -and (Get-Date) -lt $deadline) { Start-Sleep -Seconds 2 }
        }
        $script:PsqlVia = if (Have 'psql') { 'psql' } else { 'docker' }
    } else {
        if (-not (Have 'psql')) { Die 'psql not on PATH (needed to seed the runner/deployment rows) -- install PostgreSQL client tools or use LAB_PG_MODE=docker' }
        $script:PsqlVia = 'psql'
    }
    Ok "postgres ${PgHost}:$script:PgPort db=$PgDb user=$PgUser (psql via $script:PsqlVia)"
}
function Invoke-Psql($sql) { # quiet, tuples-only; throws on error
    if ($DryRun) { Dim "[dry-run] psql: $($sql -replace '\s+', ' ' | ForEach-Object { $_.Substring(0, [Math]::Min(140, $_.Length)) })"; return '' }
    # The SQL goes through a file, never argv: Windows PowerShell 5.1 does not
    # escape embedded double quotes (the JSON in the deployment INSERT) when
    # building a native command line.
    Ensure-Dir $StateDir
    $tmp = Join-Path $StateDir 'psql-cmd.sql'
    [IO.File]::WriteAllText($tmp, $sql + "`n", (New-Object Text.UTF8Encoding $false))
    if ($script:PsqlVia -eq 'docker') {
        $out = Get-Content $tmp -Raw | & docker compose -f (Join-Path $RepoRoot 'docker-compose.dashboard.yml') exec -T postgres psql -U $PgUser -d $PgDb -v ON_ERROR_STOP=1 -qtA 2>&1
    } else {
        $prev = $env:PGPASSWORD; $env:PGPASSWORD = $PgPass
        try { $out = & psql -h $PgHost -p $script:PgPort -U $PgUser -d $PgDb -v ON_ERROR_STOP=1 -qtA -f $tmp 2>&1 } finally { $env:PGPASSWORD = $prev }
    }
    if ($LASTEXITCODE -ne 0) { throw "psql failed: $out`nSQL: $sql" }
    return (($out | Out-String).Trim())
}
function Wait-Postgres {
    if ($DryRun) { return }
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) { try { Invoke-Psql 'select 1' | Out-Null; return } catch { Start-Sleep -Seconds 2 } }
    Die "PostgreSQL at ${PgHost}:$script:PgPort not answering as $PgUser/$PgDb"
}

# -- Registration (same SQL as lab/lab.sh) -------------------------------------
function Runner-Key {
    if (Test-Path $KeyFile) { return (Get-Content $KeyFile -Raw).Trim() }
    Ensure-Dir $StateDir
    $chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'.ToCharArray()
    $rng = New-Object System.Security.Cryptography.RNGCryptoServiceProvider
    $bytes = New-Object byte[] 48; $rng.GetBytes($bytes)
    $key = -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
    [IO.File]::WriteAllText($KeyFile, $key, [Text.Encoding]::ASCII)
    return $key
}
function Register-Runner($key) {
    # Standalone agent row (no project_tester/VM) -- the INSERT the create-tester
    # path performs minus the cloud VM; only the SHA-256 of the key is stored.
    $sql = @"
INSERT INTO agent (agent_id, name, api_key_hash, region, provider, project_id, status, tags)
VALUES (gen_random_uuid(), '$RunnerName', encode(sha256(convert_to('$key','UTF8')),'hex'),
        'lab', 'native-windows', '$script:ProjectId', 'offline',
        '{"lab":true,"os":"windows","ip":"127.0.0.1"}'::jsonb)
ON CONFLICT (api_key_hash) DO UPDATE SET name = EXCLUDED.name, project_id = EXCLUDED.project_id;
"@
    Invoke-Psql $sql | Out-Null
    Ok "$RunnerName registered as a standalone agent in project $script:ProjectId"
}
function Register-TargetDeployment($index, $stack, $targetHostName) {
    # COMPLETED deployment so kind=proxy configs (proxy_endpoint_id = this id,
    # proxy_stack = iis) resolve to <host>:8445 exactly like a cloud Windows endpoint.
    $name = "lab-target-$index-$stack"
    $id = Invoke-Psql "SELECT deployment_id FROM deployment WHERE name='$name' AND project_id='$script:ProjectId' LIMIT 1"
    if (-not $id) {
        $sql = @"
INSERT INTO deployment (deployment_id, name, status, config, endpoint_ips, project_id, created_at, started_at, finished_at, log)
VALUES (gen_random_uuid(), '$name', 'completed',
        '{"lab":true,"tester":{"provider":"local"},"endpoints":[{"label":"target-$index","provider":"lan","lan":{"ip":"$targetHostName","user":"lab"},"http_stacks":["$stack"],"os":"windows"}]}'::jsonb,
        '["$targetHostName"]'::jsonb, '$script:ProjectId', now(), now(), now(),
        'seeded by lab/native/lab-native.ps1 -- native windows target $index ($stack) at $targetHostName');
"@
        Invoke-Psql $sql | Out-Null
        $id = Invoke-Psql "SELECT deployment_id FROM deployment WHERE name='$name' AND project_id='$script:ProjectId' LIMIT 1"
    }
    if ($DryRun) { $id = 'dry-run-deployment' }
    Ok "target-$index ($stack) registered as deployment $id -> ${targetHostName}:8445"
    return $id
}

# -- State file for lab/validate.sh --------------------------------------------
function Save-StateEnv($h3Off) {
    Ensure-Dir $StateDir
    $hosts = @()
    foreach ($t in ($Targets -split ',')) { $hosts += $TargetHost }
    $lines = @(
        "# generated by lab/native/lab-native.ps1 -- sourced by lab/validate.sh (LAB_STATE_ENV)",
        "LAB_PROJECT_ID='$script:ProjectId'",
        "LAB_TOKEN='$script:Token'",
        "LAB_RUNNERS='1'",
        "LAB_TARGETS='$Targets'",
        "LAB_TARGET_HOSTS='$($hosts -join ',')'",
        "LAB_H3_OFF_STACKS='$h3Off'",
        "LAB_BASE_URL='$BaseUrl'",
        "LAB_ADMIN_EMAIL='$AdminEmail'",
        "LAB_ADMIN_PASSWORD='$AdminPw'",
        "LAB_NATIVE='1'"
    )
    [IO.File]::WriteAllText($StateEnv, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))
    Ok "state written -> $StateEnv"
}
function Load-StateEnv {
    $h = @{}
    if (Test-Path $StateEnv) {
        foreach ($line in Get-Content $StateEnv) { if ($line -match "^([A-Z_0-9]+)='(.*)'$") { $h[$Matches[1]] = $Matches[2] } }
    }
    return $h
}

# -- build ---------------------------------------------------------------------
function Cmd-Build {
    Note 'building Rust binaries (Windows CI feature set: tester --no-default-features --features http3,db-mssql; endpoint default)'
    if (-not (Have 'cargo')) { Die 'cargo not on PATH -- run scripts/dev-setup.ps1 (rustup + MSVC build tools + CMake)' }
    $profile = if ($DebugBuild) { @() } else { @('--release') }
    $tdir = if ($DebugBuild) { 'debug' } else { 'release' }
    Run 'cargo' (@('build') + $profile + @('-p', 'networker-tester', '--no-default-features', '--features', 'http3,db-mssql'))
    Run 'cargo' (@('build') + $profile + @('-p', 'networker-endpoint'))
    Ensure-Dir $BinDir
    foreach ($b in @('networker-tester', 'networker-endpoint')) {
        $src = Join-Path (Join-Path (Join-Path $RepoRoot 'target') $tdir) "$b$ExeSuffix"
        if ($DryRun) { Dim "[dry-run] copy $src -> $BinDir" } else { Copy-Item $src (Join-Path $BinDir "$b$ExeSuffix") -Force -ErrorAction Stop }
    }
    Note 'publishing Networker.ControlPlane (framework-dependent) + Networker.Agent (self-contained win-x64)'
    if (-not (Have 'dotnet')) { Die 'dotnet not on PATH -- .NET SDK 10 required' }
    Run 'dotnet' @('publish', (Join-Path $RepoRoot 'src\Networker.ControlPlane\Networker.ControlPlane.csproj'), '-c', 'Release', '-o', (Join-Path $PubDir 'controlplane'), '--nologo', '-v', 'q')
    Run 'dotnet' @('publish', (Join-Path $RepoRoot 'src\Networker.Agent\Networker.Agent.csproj'), '-c', 'Release', '-r', 'win-x64', '--self-contained', 'true', '-o', (Join-Path $PubDir 'agent'), '--nologo', '-v', 'q')
    if (-not $DryRun) {
        $v = Capture (Join-Path $BinDir "networker-tester$ExeSuffix") @('--version')
        Ok "networker-tester: $($v.Trim())"
        $v = Capture (Join-Path $BinDir "networker-endpoint$ExeSuffix") @('--version')
        Ok "networker-endpoint: $($v.Trim())"
    }
    Ok "build done -> $BinDir, $PubDir"
}

# -- up ------------------------------------------------------------------------
function Cmd-Up {
    if (-not $DryRun) {
        foreach ($need in @((Join-Path $BinDir "networker-tester$ExeSuffix"), (Join-Path $BinDir "networker-endpoint$ExeSuffix"), (Join-Path (Join-Path $PubDir 'controlplane') 'Networker.ControlPlane.dll'), (Join-Path (Join-Path $PubDir 'agent') "networker-agent$ExeSuffix"))) {
            if (-not (Test-Path $need)) { Die "missing $need -- run: lab-native.ps1 build" }
        }
        if (-not (Have 'dotnet')) { Die 'dotnet not on PATH' }
    }
    Ensure-Dir $LogDir

    # 1. Postgres
    Resolve-Postgres
    Wait-Postgres

    # 2. Control plane -- same env contract as lab/docker-compose.yml.
    Note "starting control plane on $BaseUrl (Production, fail-closed secrets set, migrations on startup)"
    $cpEnv = @{
        ASPNETCORE_ENVIRONMENT        = (EnvOr 'LAB_CP_ENVIRONMENT' 'Production')
        ASPNETCORE_URLS               = "http://0.0.0.0:$CpPort"
        DASHBOARD_DB_URL_NPGSQL       = "Host=$PgHost;Port=$script:PgPort;Database=$PgDb;Username=$PgUser;Password=$PgPass;Maximum Pool Size=50"
        DASHBOARD_JWT_SECRET          = $JwtSecret
        DASHBOARD_CREDENTIAL_KEY      = $CredKey
        DASHBOARD_ADMIN_EMAIL         = $AdminEmail
        DASHBOARD_ADMIN_PASSWORD      = $AdminBootPw
        DASHBOARD_PUBLIC_URL          = $BaseUrl
        DASHBOARD_BACKGROUND_SERVICES = '1'
        Logging__LogLevel__Default    = (EnvOr 'LAB_CP_LOGLEVEL' 'Information')
        'Logging__LogLevel__Microsoft.AspNetCore' = 'Warning'
        DOTNET_EnableDiagnostics      = '0'
    }
    Stop-Managed 'controlplane'
    Start-Managed 'controlplane' 'dotnet' @('Networker.ControlPlane.dll') $cpEnv (Join-Path $PubDir 'controlplane') | Out-Null
    if (-not (Wait-Http "$BaseUrl/api/health/ready" $StartupTimeout 'control plane')) {
        Get-Content (Join-Path $LogDir 'controlplane.log') -Tail 60 -ErrorAction SilentlyContinue
        Die 'control plane not ready'
    }
    Ok 'control plane ready'
    Authenticate
    Ensure-Project

    # 3. Target: bare endpoint (target-1 rust) at C:\networker (payload expects it there).
    Note 'starting networker-endpoint (8080/8443, UDP 9997-9999)'
    Stop-Managed 'endpoint'
    $epExe = Join-Path $BinDir "networker-endpoint$ExeSuffix"
    if ($IsWin) {
        Ensure-Dir $EndpointInstallDir
        $dst = Join-Path $EndpointInstallDir "networker-endpoint$ExeSuffix"
        if ($DryRun) { Dim "[dry-run] copy $epExe -> $dst" } else { Copy-Item $epExe $dst -Force -ErrorAction Stop }
        $epExe = $dst
    }
    Start-Managed 'endpoint' $epExe @() @{ RUST_LOG = 'info' } | Out-Null
    if (-not (Wait-Http 'http://127.0.0.1:8080/health' 60 'endpoint')) { Die 'endpoint did not become healthy' }
    Ok 'endpoint healthy (http://127.0.0.1:8080/health)'
    Wait-Http 'https://127.0.0.1:8443/health' 20 'endpoint https' | Out-Null

    # 4. Target: IIS (target-2 iis).
    $h3Off = ''
    if (-not $NoIis) { $h3Off = Setup-Iis }

    # 5. Runner registration + start.
    $key = Runner-Key
    Register-Runner $key
    if (-not $NoIis) { Register-TargetDeployment 2 'iis' $TargetHost | Out-Null }
    Note "starting $RunnerName (networker-agent.exe -> ws://127.0.0.1:$CpPort/ws/agent)"
    Stop-Managed 'agent'
    $agentEnv = @{
        AGENT_DASHBOARD_URL = "ws://127.0.0.1:$CpPort/ws/agent"
        AGENT_API_KEY       = $key
        AGENT_TESTER_PATH   = (Join-Path $BinDir "networker-tester$ExeSuffix")
        AGENT_NAME          = $RunnerName
        RUST_LOG            = 'info'
        DOTNET_EnableDiagnostics = '0'
    }
    Start-Managed 'agent' (Join-Path (Join-Path $PubDir 'agent') "networker-agent$ExeSuffix") @() $agentEnv (Join-Path $PubDir 'agent') | Out-Null

    Save-StateEnv $h3Off

    if (-not $DryRun) {
        Note "waiting for $RunnerName to come online (<= ${AgentTimeout}s)"
        $deadline = (Get-Date).AddSeconds($AgentTimeout); $online = @()
        while ((Get-Date) -lt $deadline) {
            try { $online = Online-Runners } catch { $online = @() }
            if ($online.Count -ge 1) { break }
            Start-Sleep -Seconds 3
        }
        if ($online.Count -lt 1) {
            Get-Content (Join-Path $LogDir 'agent.log') -Tail 40 -ErrorAction SilentlyContinue
            Die 'runner did not come online'
        }
        Ok "$($online[0].name) online (version $($online[0].version))"
    }
    Cmd-Status
}

# -- IIS target ----------------------------------------------------------------
# Returns the LAB_H3_OFF_STACKS value ('' when HTTP/3 through IIS was
# empirically confirmed with the built tester, 'iis' otherwise).
function Setup-Iis {
    Note "setting up IIS target (mode=$IisSetup, host=$TargetHost)"
    if (-not $IsWin -and -not $DryRun) { Warn 'IIS is Windows-only -- skipping (use -NoIis)'; return 'iis' }
    $payloadLog = Join-Path $LogDir 'iis-setup.log'
    $rebootNeeded = $false
    if ($IisSetup -eq 'installer') {
        # install.ps1 -Setup iis: HTTP-only stub today (site on 8082, no 8445).
        Warn 'install.ps1 -Setup iis is an HTTP-only stub (8082, no HTTPS 8445 / ARR / HTTP/3) -- the proxy matrix on 8445 will not pass; this mode exists to keep that gap visible'
        $ps = Find-WindowsPowerShell
        $installer = Join-Path $RepoRoot 'install.ps1'
        if ($DryRun) { Dim "[dry-run] powershell.exe -NoProfile -ExecutionPolicy Bypass -File $installer -Setup iis" }
        elseif (-not $ps) { Die 'powershell.exe (Windows PowerShell 5.1) not found' }
        else { & $ps -NoProfile -ExecutionPolicy Bypass -File $installer -Setup iis 2>&1 | Tee-Object -FilePath $payloadLog | ForEach-Object { Dim $_ } }
    } else {
        # The payload every cloud Windows endpoint VM gets (Azure run-command,
        # AWS UserData, GCP startup script all reuse install.sh _iis_setup_powershell).
        $bash = Find-Bash
        if (-not $bash) { Die 'Git Bash not found (needed to render the IIS payload from install.sh) -- install Git for Windows or set LAB_BASH' }
        $fq = if ($Fqdn) { $Fqdn } else { '' }
        $script = "cd '$(To-BashPath $RepoRoot)' && source ./install.sh && set +euo pipefail && _iis_setup_powershell '$fq'"
        $payloadPath = Join-Path $StateDir 'iis-setup.ps1'
        if ($DryRun) { Dim "[dry-run] $bash -c `"$script`" > $payloadPath" }
        else {
            $payload = & $bash -c $script 2>&1 | Out-String
            if ($LASTEXITCODE -ne 0 -or $payload -notmatch 'networker-iis') { Die "could not render the IIS payload from install.sh: $payload" }
            # BOM so Windows PowerShell 5.1 reads the UTF-8 payload correctly.
            [IO.File]::WriteAllText($payloadPath, $payload, (New-Object Text.UTF8Encoding $true))
            Ok "IIS payload rendered ($((($payload -split "`n").Count)) lines) -> $payloadPath"
            $ps = Find-WindowsPowerShell
            if (-not $ps) { Die 'powershell.exe (Windows PowerShell 5.1) not found -- required for Install-WindowsFeature/WebAdministration' }
            Note 'running IIS payload (Install-WindowsFeature Web-Server + URL Rewrite + ARR + site + certs) -- a few minutes on a fresh host'
            & $ps -NoProfile -ExecutionPolicy Bypass -File $payloadPath 2>&1 | Tee-Object -FilePath $payloadLog | ForEach-Object { Dim $_ }
            $rc = $LASTEXITCODE
            if ($rc -ne 0) { Warn "IIS payload exited $rc -- see $payloadLog (continuing to probe what came up)" }
            if ((Get-Content $payloadLog -Raw -ErrorAction SilentlyContinue) -match 'REBOOT_NEEDED') { $rebootNeeded = $true }
        }
    }
    if ($DryRun) { Dim '[dry-run] wait 8082/8445, probe h3'; return 'iis' }

    $httpOk  = Wait-Http 'http://127.0.0.1:8082/health' $IisTimeout 'IIS http'
    $httpsOk = Wait-Http "https://${TargetHost}:8445/health" 30 'IIS https'
    if ($httpOk)  { Ok 'IIS serving http://127.0.0.1:8082/health' } else { Warn 'IIS HTTP listener (8082) not serving' }
    if ($httpsOk) { Ok "IIS serving https://${TargetHost}:8445/health (ARR -> endpoint)" } else {
        Warn "IIS HTTPS listener (8445) not serving -- validate phase 2 (iis) will fail; diagnostics:"
        Capture 'netsh' @('http', 'show', 'sslcert') | ForEach-Object { Dim $_ }
        Capture 'netsh' @('http', 'show', 'urlacl') | Select-Object -First 5 | ForEach-Object { Dim $_ }
    }

    # HTTP/3 through IIS -- honest detection, never assumed:
    #  * http.sys binds QUIC on SNI hostname bindings only -> IP-literal targets
    #    (the default) get H1/H2 only, by design;
    #  * the EnableHttp3 registry key needs a reboot (REBOOT_NEEDED above) --
    #    impossible on a hosted runner; try restarting http.sys when a hostname
    #    is in play (that is the only case where it can matter).
    if ($TargetHost -match '^\d+\.\d+\.\d+\.\d+$') {
        Warn "IIS target is the IP literal $TargetHost -> HTTP/3 through IIS is unavailable BY DESIGN (SNI hostname bindings only); h3 modes excluded for iis (LAB_H3_OFF_STACKS=iis). Pass -Fqdn <hostname> to try."
        return 'iis'
    }
    if ($rebootNeeded -and (EnvOr 'LAB_IIS_HTTP_RESTART' '1') -eq '1') {
        Note 'HTTP/3 registry keys were just set (REBOOT_NEEDED) -- restarting http.sys instead of rebooting (best effort)'
        try {
            & net stop http /y 2>&1 | ForEach-Object { Dim $_ }
            & net start w3svc 2>&1 | ForEach-Object { Dim $_ }
            Start-Sleep -Seconds 3
        } catch { Warn "http.sys restart failed: $($_.Exception.Message)" }
    }
    $tester = Join-Path $BinDir "networker-tester$ExeSuffix"
    $probeOut = Join-Path $LogDir 'iis-h3-probe.json'
    $probeErr = Join-Path $LogDir 'iis-h3-probe.err.log'
    $h3ok = $false
    try {
        & $tester --target "https://${TargetHost}:8445/health" --modes http3 --runs 2 --timeout 8 --insecure --json-stdout 1> $probeOut 2> $probeErr
        $j = Get-Content $probeOut -Raw | ConvertFrom-Json
        $okc = @($j.attempts | Where-Object { $_.success -eq $true }).Count
        $h3ok = ($okc -gt 0)
        Dim "http3 probe: $okc/$(@($j.attempts).Count) attempt(s) succeeded"
    } catch { Dim "http3 probe could not be parsed: $($_.Exception.Message)" }
    if ($h3ok) { Ok "HTTP/3 through IIS CONFIRMED (https://${TargetHost}:8445) -- h3 modes stay in the iis matrix"; return '' }
    $why = if ($rebootNeeded) { 'HTTP/3 registry keys were set in this session (reboot pending)' } else { 'see iis-h3-probe.err.log' }
    Warn "HTTP/3 through IIS NOT available on this host ($why) -- h3 modes excluded for iis (LAB_H3_OFF_STACKS=iis)"
    return 'iis'
}

# -- validate ------------------------------------------------------------------
function Cmd-Validate {
    if (-not (Test-Path $StateEnv) -and -not $DryRun) { Die "no lab state ($StateEnv) -- run: lab-native.ps1 up" }
    $bash = Find-Bash
    if (-not $bash) { Die 'Git Bash not found (lab/validate.sh needs bash + curl + jq) -- install Git for Windows (+ winget install jqlang.jq) or set LAB_BASH' }
    $validate = To-BashPath (Join-Path $LabDir 'validate.sh')
    $stateBash = To-BashPath $StateEnv
    $argStr = ($ValidateArgs | ForEach-Object { "'" + ($_ -replace "'", "'\''") + "'" }) -join ' '
    $script = "export LAB_STATE_ENV='$stateBash'; export MSYS_NO_PATHCONV=1; bash '$validate' $argStr"
    Note "validate: $bash -c `"$script`""
    if ($DryRun) { return }
    Ensure-Dir $LogDir
    & $bash -c $script 2>&1 | Tee-Object -FilePath (Join-Path $LogDir 'validate.log')
    $rc = $LASTEXITCODE
    if ($rc -ne 0) { Write-Host "  [x]    validate exited $rc" -ForegroundColor Red; exit $rc }
    Ok 'validate passed'
}

# -- status / logs / env / down ------------------------------------------------
function Cmd-Status {
    $st = Load-StateEnv
    $pids = Load-Pids
    Write-Host ''
    Write-Host 'networker native windows lab' -ForegroundColor White
    $health = 'unreachable'
    if (-not $DryRun) { try { $health = (Invoke-WebRequest -Uri "$BaseUrl/api/health/ready" -UseBasicParsing -TimeoutSec 3).Content; if ($health.Length -gt 60) { $health = $health.Substring(0, 60) } } catch { } }
    Write-Host "  control plane : $BaseUrl   (health: $health)"
    Write-Host "  project       : $(if ($st.ContainsKey('LAB_PROJECT_ID')) { $st['LAB_PROJECT_ID'] } else { '?' })"
    Write-Host "  targets       : $(if ($st.ContainsKey('LAB_TARGETS')) { $st['LAB_TARGETS'] } else { $Targets }) @ $TargetHost   (rust 8080/8443, iis 8082/8445$(if ($st.ContainsKey('LAB_H3_OFF_STACKS') -and $st['LAB_H3_OFF_STACKS']) { "; h3 excluded for: $($st['LAB_H3_OFF_STACKS'])" }))"
    Write-Host "  processes     : $(($pids.Keys | ForEach-Object { "$_=$($pids[$_])" }) -join '  ')"
    if (-not $DryRun -and $st.ContainsKey('LAB_TOKEN')) {
        $script:Token = $st['LAB_TOKEN']; $script:ProjectId = $st['LAB_PROJECT_ID']
        try {
            $rs = Online-Runners
            foreach ($r in $rs) { Write-Host "  runner        : $($r.name) $($r.status) v$($r.version) last=$($r.last_heartbeat)" }
            if ($rs.Count -eq 0) { Write-Host '  runner        : (none online)' }
        } catch { Write-Host "  runner        : (api error: $($_.Exception.Message))" }
    }
    Write-Host "  logs          : $LogDir"
    Write-Host ''
    Write-Host '  next: .\lab\native\lab-native.ps1 validate' -ForegroundColor DarkGray
}
function Cmd-Logs {
    if (-not (Test-Path $LogDir)) { Warn 'no logs yet'; return }
    foreach ($f in Get-ChildItem $LogDir -File | Sort-Object Name) {
        Write-Host "-- $($f.Name) ($($f.Length) B) --" -ForegroundColor White
        Get-Content $f.FullName -Tail 40 -ErrorAction SilentlyContinue
    }
}
function Cmd-Env { if (Test-Path $StateEnv) { Get-Content $StateEnv } else { Warn 'no state yet' } }
function Cmd-Down {
    foreach ($n in @('agent', 'endpoint', 'controlplane')) { Stop-Managed $n }
    # Stray tester processes spawned by the agent for in-flight runs.
    if (-not $DryRun -and $IsWin) { Get-Process -Name 'networker-tester' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue }
    Ok 'lab stopped (state, DB rows and IIS site kept; delete lab/native/.state to forget the runner key/token)'
}

# -- main ----------------------------------------------------------------------
switch ($Command) {
    'build'    { Cmd-Build }
    'up'       { Cmd-Up }
    'validate' { Cmd-Validate }
    'status'   { Cmd-Status }
    'logs'     { Cmd-Logs }
    'env'      { Cmd-Env }
    'down'     { Cmd-Down }
    'all'      { Cmd-Build; Cmd-Up; Cmd-Validate }
    default    { Get-Help $PSCommandPath -Detailed }
}
