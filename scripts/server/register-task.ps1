# Ver7.0 サーバー: 窓口を、ログインしなくても、起動時に自動で起こすタスクとして登録する。
# 管理者権限のPowerShellで実行する。外すときは -Remove。
# 窓口が落ちたら1分おきに起こし直す。Tailscaleのアドレスがまだ無い起動直後は、起動に失敗して、次の1分で再試行される。
param(
    [string]$Root = 'C:\ShioriServer',
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'
$name = 'ShioriServer'
if ($Remove) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false
    Write-Host "外した: $name"
    return
}
$script = Join-Path $Root 'scripts\start-server.ps1'
if (-not (Test-Path $script)) { throw "見つからない: $script" }
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$script`" -Root `"$Root`""
# 起動時に始め、さらに1分おきに起動を試みる。動いている間は、重ねて起動しない(IgnoreNew)。
$triggerBoot = New-ScheduledTaskTrigger -AtStartup
$triggerRepeat = New-ScheduledTaskTrigger -Once -At (Get-Date) `
    -RepetitionInterval (New-TimeSpan -Minutes 1) -RepetitionDuration (New-TimeSpan -Days 3650)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $action -Trigger @($triggerBoot, $triggerRepeat) -Settings $settings -Principal $principal -Force | Out-Null
Write-Host "登録した: $name (起動時に自動で始まる。いま始めるには: Start-ScheduledTask $name)"
