# Ver7.0 Phase 0 (4-5 自動で戻る): 窓口を、ログインしなくても起動時に自動で起こすタスクとして登録する
# 管理者権限のPowerShellで実行する。外すときは -Remove。
# 窓口が落ちたら1分おきに起こし直す。Tailscaleのアドレスがまだ無い起動直後は、起動に失敗して再試行される。
param(
    [ValidateSet('x1', 'x5', 'x10')][string]$Size = 'x1',
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$name = 'ShioriProtoServer'
if ($Remove) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false
    Write-Host "外した: $name"
    return
}
$script = "C:\shiori-proto\scripts\start-server.ps1"
if (-not (Test-Path $script)) { throw "見つからない: $script" }
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$script`" -Size $Size"
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
Write-Host "登録した: $name (起動時に自動で始まる。いま始めるには: Start-ScheduledTask $name)"
