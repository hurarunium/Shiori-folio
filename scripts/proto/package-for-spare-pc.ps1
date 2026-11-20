# Ver7.0 Phase 0: 余りのPCへ渡す一式を、1つのzipにまとめる(開発機で実行)
# 前提: services\shiori-proto-server で cargo build --release 済み、C:\shiori-proto に runtime と x1 / x5 / x10 がある
# 本物の portable\ や library には触れない。
$ErrorActionPreference = 'Stop'
$repo = Split-Path (Split-Path $PSScriptRoot)
$root = 'C:\shiori-proto'
$rel  = Join-Path $repo 'services\shiori-proto-server\target\release'

foreach ($p in @("$root\runtime", "$root\x1", "$rel\shiori-proto-server.exe", "$rel\mcp_relay.exe")) {
    if (-not (Test-Path $p)) { throw "見つからない: $p" }
}

# 小さい物(実行ファイルとスクリプト)を、zipの元になる場所へそろえる
New-Item -ItemType Directory -Force "$root\bin", "$root\scripts" | Out-Null
Copy-Item "$rel\shiori-proto-server.exe", "$rel\mcp_relay.exe" "$root\bin\" -Force
Copy-Item "$PSScriptRoot\spare\*.ps1" "$root\scripts\" -Force

$items = @('runtime', 'bin', 'scripts', 'x1')
foreach ($x in 'x5', 'x10') { if (Test-Path "$root\$x") { $items += $x } }

$zip = "$root\shiori-proto-package.zip"
if (Test-Path $zip) { Remove-Item $zip }
Push-Location $root
try { tar -a -cf $zip @items } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw "zipを作れなかった" }

$mb = [math]::Round((Get-Item $zip).Length / 1MB)
Write-Host "作った: $zip ($mb MB)"
Write-Host "中身: $($items -join ', ')"
Write-Host "次: tailscale file cp `"$zip`" <余りのPCの端末名>:"
