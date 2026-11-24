# 余りのPCで動かす点検用(読み取りだけ。何も書き換えない)
# 目的: 夜のあいだに眠った時刻・復帰した時刻・復帰の原因と、眠らせない要求が効いているかを見る。
# 使い方: 余りのPCで、管理者のPowerShellから
#   powershell -ExecutionPolicy Bypass -File check-sleep.ps1
param([int]$HoursBack = 24)

$since = (Get-Date).AddHours(-$HoursBack)
Write-Host "=== 現在時刻: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') / 調べる範囲: $since 以降 ===" -ForegroundColor Cyan

Write-Host "`n--- 1. サーバーのプロセスの開始時刻 ---" -ForegroundColor Cyan
Get-Process -Name 'shiori-server' -ErrorAction SilentlyContinue | Select-Object Name, Id, StartTime | Format-Table -AutoSize

Write-Host "--- 2. いま出ている「眠らせない要求」(shiori-server が EXECUTION か SYSTEM に出ていれば効いている) ---" -ForegroundColor Cyan
powercfg /requests

Write-Host "--- 3. スリープに入った・出た記録(Kernel-Power: 42=スリープ入り 107=復帰 506/507=モダンスタンバイの入り/出) ---" -ForegroundColor Cyan
try {
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Kernel-Power'; Id = 42, 107, 506, 507; StartTime = $since } -ErrorAction Stop |
        Sort-Object TimeCreated |
        Select-Object TimeCreated, Id | Format-Table -AutoSize
} catch { Write-Host "(該当なし、または読めなかった: $($_.Exception.Message))" }

Write-Host "--- 4. 復帰の原因(Power-Troubleshooter ID 1。「Wake Source」の行を見る) ---" -ForegroundColor Cyan
try {
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Power-Troubleshooter'; Id = 1; StartTime = $since } -ErrorAction Stop |
        Sort-Object TimeCreated |
        Select-Object TimeCreated, Message | Format-List
} catch { Write-Host "(該当なし、または読めなかった: $($_.Exception.Message))" }

Write-Host "--- 5. 電源の状態の変化(Kernel-Power ID 105=電源の切り替え、109=シャットダウン開始、41=異常終了) ---" -ForegroundColor Cyan
try {
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Microsoft-Windows-Kernel-Power'; Id = 41, 105, 109; StartTime = $since } -ErrorAction Stop |
        Sort-Object TimeCreated |
        Select-Object TimeCreated, Id | Format-Table -AutoSize
} catch { Write-Host "(該当なし)" }

Write-Host "--- 6. スリープ調査のレポートを作る(管理者のとき。作るだけで、設定は変えない) ---" -ForegroundColor Cyan
$report = Join-Path $env:TEMP 'sleepstudy.html'
powercfg /sleepstudy /output $report 2>&1 | Out-Host
if (Test-Path $report) { Write-Host "レポート: $report (ブラウザで開くと、画面オフ中の電力の動きが見える)" }

Write-Host "--- 7. Tailscale の状態 ---" -ForegroundColor Cyan
tailscale status 2>&1 | Select-Object -First 8
