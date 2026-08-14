# Removes the LunimaAgentLoop scheduled task.
$ErrorActionPreference = 'Stop'
Unregister-ScheduledTask -TaskName 'LunimaAgentLoop' -Confirm:$false
Write-Host "Scheduled task 'LunimaAgentLoop' removed."
