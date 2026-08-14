# One-command setup for a new machine (Windows, incl. ARM64):
# checks prerequisites, builds, inits (clone + integration branch), registers the scheduler.
#Usage:  git clone https://github.com/aignermax/lunima-agent-loop; cd lunima-agent-loop
#        scripts\Setup-Machine.ps1            # full setup incl. hourly scheduled task
#        scripts\Setup-Machine.ps1 -NoRegister  # build + init only, no scheduler
#Requires -Version 5.1
[CmdletBinding()]
param([switch]$NoRegister)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

Write-Host "== Checking prerequisites =="
$sdks = dotnet --list-sdks 2>$null
if (-not ($sdks | Where-Object { $_ -match '^10\.' })) { throw ".NET 10 SDK missing: https://dot.net/download" }
Write-Host "  dotnet SDK: ok"

$kimi = Get-Command kimi -ErrorAction SilentlyContinue
if (-not $kimi) { throw "kimi CLI not on PATH (https://www.kimi.com/code/docs/en/) — install, then 'kimi login'" }
Write-Host "  kimi CLI:   ok ($($kimi.Source))"

$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) { throw "GitHub CLI not on PATH (https://cli.github.com/) — install, then 'gh auth login'" }
gh auth status 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "gh not authenticated — run 'gh auth login'" }
Write-Host "  gh CLI:     ok (authenticated)"

$config = Join-Path $root 'agent-loop.json'
if (-not (Test-Path $config)) {
    Copy-Item (Join-Path $root 'agent-loop.example.json') $config
    Write-Host ""
    Write-Host "Created agent-loop.json from the example."
    Write-Host "Edit it if this machine needs a different clonePath/caps, then re-run this script."
    exit 0
}

Write-Host "== Init (clone target repo, ensure integration branch) =="
dotnet build (Join-Path $root 'lunima-agent-loop.csproj') -c Release --nologo
if ($LASTEXITCODE -ne 0) { throw "build failed" }
& (Join-Path $root 'bin\Release\net10.0\lunima-agent-loop.exe') init
if ($LASTEXITCODE -ne 0) { throw "init failed" }

if ($NoRegister) {
    Write-Host "== Done (no scheduler registered; -NoRegister) =="
} else {
    & (Join-Path $PSScriptRoot 'Register-AgentLoop.ps1')
    Write-Host "== Done =="
}
Write-Host ""
Write-Host "NOTE: every machine runs against the SAME GitHub repo and the SAME Kimi account quota."
Write-Host "Issue claims (agent-running label) prevent duplicate work, but two machines burn budget twice as fast."
