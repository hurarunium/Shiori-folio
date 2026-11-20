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
# 起動時に始め、さらに1分おきに起動を試みる。
# 窓口が(強制終了などで)終わると、スクリプトは「正常終了」になり、「失敗したら再起動」では戻らない。
# そこで、繰り返しの起動条件で、動いていなければ始め直す。動いている間は、重ねて起動しない(IgnoreNew)。
$triggerBoot = New-ScheduledTaskTrigger -AtStartup
$triggerRepeat = New-ScheduledTaskTrigger -Once -At (Get-Date) `
    -RepetitionInterval (New-TimeSpan -Minutes 1) -RepetitionDuration (New-TimeSpan -Days 3650)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -MultipleInstances IgnoreNew `
    -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $action -Trigger @($triggerBoot, $triggerRepeat) -Settings $settings -Principal $principal -Force | Out-Null
Write-Host "登録した: $name (起動時に自動で始まる。いま始めるには: Start-ScheduledTask $name)"
