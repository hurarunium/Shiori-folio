# Ver7.0 サーバー: 配置の点検(読み取りだけ。何も変えない)。
# 時計の同期(日誌の日の区切りに効く)・Tailscale・窓口の応答・空き容量を確かめる。
param(
    [string]$Root = 'C:\ShioriServer',
    [int]$Port = 8090
)
$ng = 0
function Check($label, $ok, $detail) {
    if ($ok) { Write-Host "OK  $label  $detail" } else { Write-Host "NG  $label  $detail"; $script:ng++ }
}
# 時計: 日誌の「午前5時で日が変わる」は、サーバーの時計が基準。ずれていると、日付を取り違える。
$w32 = & w32tm /query /status 2>&1 | Out-String
$offset = [regex]::Match($w32, '(?m)^.*(位相のオフセット|Phase Offset)\s*:\s*([-0-9.,]+)').Groups[2].Value
if (-not $w32.Trim()) {
    Check '時計の同期' $false 'w32tm の結果が取れない(管理者権限か、Windows Timeサービスを確認する)'
} else {
    Check '時計の同期' ($w32 -match '(ソース|Source)' -and $w32 -notmatch 'Local CMOS Clock|Free-running') "(オフセット: $offset 秒)"
}
$tz = (Get-TimeZone).Id
Check 'タイムゾーン(日本時間)' ($tz -eq 'Tokyo Standard Time') $tz
# Tailscale
$ip = $null
try { $ip = (& tailscale ip -4 | Select-Object -First 1) } catch {}
Check 'Tailscaleのアドレス' ([bool]$ip) "$ip"
# 窓口
# Tailscaleのアドレスで待っている(本番)。手元だけで試すときは 127.0.0.1 で待っているので、そちらも試す。
$answered = $false
foreach ($h in @($ip, '127.0.0.1') | Where-Object { $_ }) {
    try {
        $r = Invoke-RestMethod -Uri "http://${h}:$Port/health" -TimeoutSec 5
        Check '窓口の応答' ($r.status -eq 'ok') "$h api=$($r.api)"
        $answered = $true
        break
    } catch { }
}
if (-not $answered) { Check '窓口の応答' $false "つながらない(Port $Port)" }
# 正本の置き場と空き容量
foreach ($d in 'library', 'knowledge', 'tasks', 'data') {
    Check "置き場: $d" (Test-Path (Join-Path $Root $d)) (Join-Path $Root $d)
}
$drive = (Get-Item $Root -ErrorAction SilentlyContinue).PSDrive
if ($drive) { Check '空き容量(5GB以上)' ($drive.Free -gt 5GB) ("{0:N1} GB" -f ($drive.Free / 1GB)) }
if ($ng -eq 0) { Write-Host '点検: すべてOK' } else { Write-Host "点検: NG $ng 件"; exit 1 }
