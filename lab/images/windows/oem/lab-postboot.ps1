# Runs once at the boot AFTER lab-setup.ps1 rebooted the VM (schtasks ONSTART
# as SYSTEM, self-deleting): waits for the endpoint (:8080, ONSTART task) and
# the stacks, then flips the status file to "ready" for the host poller.
$ErrorActionPreference = 'Continue'
$LabDir = "C:\lab"; $Share = "\\host.lan\Data"
function Log($m) {
    $line = "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')  $m"
    Add-Content -Path "$LabDir\lab-setup.log" -Value $line
    try { Add-Content -Path "$Share\lab-setup.log" -Value $line -ErrorAction Stop } catch { }
}
function Set-LabStatus($s) {
    Set-Content -Path "$LabDir\status" -Value $s
    try { Set-Content -Path "$Share\status" -Value $s -ErrorAction Stop } catch { }
    Log "STATUS $s"
}
function Wait-Http($url, $seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        try { $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 5; if ($r.StatusCode -eq 200) { return $true } } catch { }
        Start-Sleep -Seconds 3
    }
    return $false
}
Log "=== post-boot check"
$ok = $true
if (Wait-Http "http://127.0.0.1:8080/health" 300) { Log "endpoint :8080 up after reboot" } else { Log "WARN endpoint :8080 not up 300s after reboot"; $ok = $false }
if (Get-Website -Name "networker-iis" -ErrorAction SilentlyContinue) {
    if (Wait-Http "http://127.0.0.1:8082/health" 120) { Log "IIS :8082 up after reboot" } else { Log "WARN IIS :8082 not up after reboot"; $ok = $false }
    $h3 = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Services\HTTP\Parameters' -Name EnableHttp3 -ErrorAction SilentlyContinue).EnableHttp3
    Log "http.sys EnableHttp3=$h3"
}
if ($ok) { Set-LabStatus "ready" } else { Set-LabStatus "failed:post-boot" }
schtasks /Delete /TN 'NetworkerLabPostBoot' /F 2>&1 | Out-Null
