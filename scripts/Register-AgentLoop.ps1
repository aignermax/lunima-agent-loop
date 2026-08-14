# Registers a windowless scheduled task that runs `lunima-agent-loop run` every 4 hours.
# Runs while the user is logged on — including on the lock screen. Survives reboots.
#Requires -Version 5.1
[CmdletBinding()]
param(
    [int]$IntervalHours = 4
)
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
Write-Host "Publishing lunima-agent-loop ..."
dotnet publish (Join-Path $root 'lunima-agent-loop.csproj') -c Release -o (Join-Path $root 'publish') --nologo | Out-Null
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed" }

$exe = Join-Path $root 'publish\lunima-agent-loop.exe'
if (-not (Test-Path $exe)) { throw "Expected exe not found: $exe" }

$action   = New-ScheduledTaskAction -Execute $exe -Argument 'run' -WorkingDirectory $root
$trigger  = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(15) `
              -RepetitionInterval (New-TimeSpan -Hours $IntervalHours) `
              -RepetitionDuration (New-TimeSpan -Days 3650)
$settings = New-ScheduledTaskSettingsSet `
              -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
              -StartWhenAvailable -MultipleInstances IgnoreNew `
              -ExecutionTimeLimit (New-TimeSpan -Hours ($IntervalHours + 1))

Register-ScheduledTask -TaskName 'LunimaAgentLoop' -Action $action -Trigger $trigger `
    -Settings $settings -Force `
    -Description 'Autonomous Lunima agent loop (Kimi Code CLI): works agent-task issues into the dev-ki branch + one daily product-owner pass.' | Out-Null

Write-Host ""
Write-Host "Registered scheduled task 'LunimaAgentLoop' (every $IntervalHours h, first run in ~15 min)."
Write-Host "  Pause:    set 'enabled' to false in agent-loop.json   (keeps the task, skips all work)"
Write-Host "  Disable:  Disable-ScheduledTask -TaskName LunimaAgentLoop"
Write-Host "  Remove:   scripts\Unregister-AgentLoop.ps1"
Write-Host "  Run now:  $exe run"
