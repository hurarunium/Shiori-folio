# Ver7.0 Phase 0 (4-7): 窓口の世代のzipを、まだ無いものだけ、手元のフォルダ(SSDの代わり)へ写す
# 写したあと、zip全体のSHA-256を、窓口の一覧の値と照らす。合わなければ消して失敗にする。
# 例: .\backup-copy.ps1 -Url http://100.105.22.70:9090 -Token <トークン> -Dest D:\shiori-backups
param(
    [Parameter(Mandatory)][string]$Url,
    [Parameter(Mandatory)][string]$Token,
    [Parameter(Mandatory)][string]$Dest
)
$ErrorActionPreference = 'Stop'
$Url = $Url.TrimEnd('/')
$h = @{ Authorization = "Bearer $Token" }
New-Item -ItemType Directory -Force $Dest | Out-Null
$list = Invoke-RestMethod "$Url/backups" -Headers $h -TimeoutSec 30
$copied = 0; $skipped = 0; $failed = 0
foreach ($g in $list) {
    $path = Join-Path $Dest $g.name
    if (Test-Path $path) { $skipped++; continue }
    $tmp = "$path.part"
    Invoke-WebRequest "$Url/backup?name=$([uri]::EscapeDataString($g.name))" -Headers $h -OutFile $tmp -TimeoutSec 600
    $sha = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
    if ($sha -ne $g.sha256) {
        Remove-Item $tmp -Force
        Write-Host "照合が合わない、捨てた: $($g.name)"
        $failed++
        continue
    }
    Move-Item $tmp $path
    Write-Host "写した: $($g.name) ($([math]::Round($g.size / 1KB)) KB)"
    $copied++
}
Write-Host "写した $copied / すでにあった $skipped / 失敗 $failed(窓口の世代: $($list.Count))"
if ($failed -gt 0) { exit 1 }
