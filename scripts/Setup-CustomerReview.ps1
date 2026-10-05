# Configure the customer role after reviewing/merging its PR. Does not launch the desktop test.
#Requires -Version 5.1
[CmdletBinding()]
param([string]$Python = 'python', [string]$Model = 'claude-fable-5-1')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$configPath = Join-Path $root 'agent-loop.json'
if (-not (Test-Path -LiteralPath $configPath)) { throw 'Initialize agent-loop.json first.' }
if (-not $env:ANTHROPIC_API_KEY) { throw 'Provide ANTHROPIC_API_KEY to the test account and scheduled task first. No key is stored by this script.' }
$venv = Join-Path $root '.customer-venv'
& $Python -m venv $venv
if ($LASTEXITCODE -ne 0) { throw 'Could not create customer Python environment.' }
$customerPython = Join-Path $venv 'Scripts\python.exe'
& $customerPython -m pip install -r (Join-Path $root 'tools\ux-tester\requirements.txt')
if ($LASTEXITCODE -ne 0) { throw 'Customer dependencies could not be installed.' }
& $customerPython -c 'import anthropic, mss, pyautogui, pygetwindow, PIL'
if ($LASTEXITCODE -ne 0) { throw 'Customer dependencies could not be imported.' }
dotnet publish (Join-Path $root 'lunima-agent-loop.csproj') -c Release -o (Join-Path $root 'publish') --nologo
if ($LASTEXITCODE -ne 0) { throw 'Agent loop build failed; customer role not enabled.' }
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$config | Add-Member -NotePropertyName customerPython -NotePropertyValue $customerPython -Force
$config | Add-Member -NotePropertyName customerModel -NotePropertyValue $Model -Force
$config | Add-Member -NotePropertyName customerEnabled -NotePropertyValue $true -Force
$json = $config | ConvertTo-Json -Depth 10
[System.IO.File]::WriteAllText($configPath, $json, (New-Object System.Text.UTF8Encoding $false))
Write-Host 'Customer role configured. On a dedicated unlocked desktop run: publish\lunima-agent-loop.exe customer'
Write-Host 'Scheduled run/own passes now refresh customer feedback before the PO. Locked desktops produce BLOCKED.'
