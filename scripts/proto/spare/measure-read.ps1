# Ver7.0 Phase 0 (4-1): 窓口から記事を1件読む時間を測る(Windowsの端末で実行)
# 例: .\measure-read.ps1 -Url http://100.105.22.70:9090 -Token <トークン> -Count 20
param(
    [Parameter(Mandatory)][string]$Url,
    [Parameter(Mandatory)][string]$Token,
    [string]$Path = '10-projects/proj-alpha/memory.md',
    [int]$Count = 20
)
$h = @{ Authorization = "Bearer $Token" }
$uri = "$($Url.TrimEnd('/'))/doc?path=$([uri]::EscapeDataString($Path))"
$times = @()
for ($i = 1; $i -le $Count; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try { Invoke-RestMethod -Uri $uri -Headers $h -TimeoutSec 30 | Out-Null; $ok = $true } catch { $ok = $false }
    $sw.Stop()
    if ($ok) { $times += $sw.Elapsed.TotalMilliseconds } else { Write-Host "失敗: $i 回目" }
    Start-Sleep -Milliseconds 300
}
if ($times.Count -gt 0) {
    $s = $times | Sort-Object
    "成功 {0}/{1}  最小 {2:N0}ms  中央値 {3:N0}ms  最大 {4:N0}ms" -f $times.Count, $Count, $s[0], $s[[int]($s.Count / 2)], $s[-1]
}
