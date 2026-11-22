# Ver7.0 サーバー: 手元(開発機)で、目視テストをするための試験環境を作る。
# ダミーのlibrary(V7の置き場)・サーバーの設定・トークン・端末側(portable)を作り、続きのコマンドを表示する。
# 本物のSSD・libraryには触れない。作り直すときは -Clean。
# 使い方(リポジトリの直下で、先にビルドしておく):
#   (cd services\shiori-server; cargo build)
#   (cd src-tauri; cargo build --bin shiori_save --bin shiori_task --bin mcp_server)
#   powershell -ExecutionPolicy Bypass -File scripts\server\make-test-env.ps1
param(
    [string]$Root = 'C:\shiori-server-test',
    [int]$Port = 18090,
    [string]$Profile = 'debug',
    [switch]$Clean
)
$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$server = Join-Path $repo "services\shiori-server\target\$Profile\shiori-server.exe"
$bins = 'shiori_save', 'shiori_task', 'mcp_server' | ForEach-Object { Join-Path $repo "src-tauri\target\$Profile\$_.exe" }
foreach ($f in @($server) + $bins) { if (-not (Test-Path $f)) { throw "ビルドされていない: $f" } }
if ($Clean -and (Test-Path $Root)) { Remove-Item -Recurse -Force $Root }
if (Test-Path $Root) { throw "もうある: $Root(作り直すなら -Clean)" }

$utf8 = New-Object System.Text.UTF8Encoding($false)   # BOMなし(JSON・YAMLの読み取りが失敗しないように)
function Put($path, $text) { New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null; [IO.File]::WriteAllText($path, $text, $utf8) }

$S = Join-Path $Root 'server'
$P = Join-Path $Root 'portable'
Put "$S\library\_system\projects.yaml" "# 台帳(試験用)`nprojects:`n  - id: shiori`n    status: confirmed`n  - id: common`n    status: confirmed`n"
Put "$S\library\_system\tags.yaml" "# タグ(試験用)`ntags:`n  - canonical: 詩織`n    aliases: [shiori]`n    status: confirmed`n"
Put "$S\library\_system\tier1-profile.md" "# プロファイル(試験用)`n"
$cfg = [ordered]@{
    bind = "127.0.0.1:$Port"; library = "$S\library"; knowledge = "$S\knowledge"; tasks = "$S\tasks"; data = "$S\data"
    no_children = $true; layout = 'v7'
} | ConvertTo-Json
Put "$S\server.json" $cfg
New-Item -ItemType Directory -Force "$S\data", "$P\bin", "$P\data", "$P\work" | Out-Null

$tokenOut = & $server token add win --data "$S\data"
$token = ($tokenOut | Select-Object -Last 1).Trim()
Put "$P\config.json" '{}'
Put "$P\data\server.json" ((@{ url = "http://127.0.0.1:$Port"; token = $token } | ConvertTo-Json))
foreach ($b in $bins) { Copy-Item $b "$P\bin\" }

Write-Host "作った: $Root"
Write-Host ''
Write-Host '--- 1. サーバーを起動する(別のPowerShellの窓で。止めるときは Ctrl+C) ---'
Write-Host "  & `"$server`" serve --config `"$S\server.json`""
Write-Host ''
Write-Host '--- 2. 元気かを確かめる(この窓で) ---'
Write-Host "  Invoke-RestMethod http://127.0.0.1:$Port/health"
Write-Host ''
Write-Host '--- 3. 保存・日誌・タスクを試す(例) ---'
Write-Host "  `$w = `"$P\work`""
Write-Host '  @"'
Write-Host '---'
Write-Host 'title: 試験の記事'
Write-Host 'type: project'
Write-Host 'kind: resource'
Write-Host 'project: shiori'
Write-Host 'tags: [詩織]'
Write-Host '---'
Write-Host '本文です。'
Write-Host '"@ | Set-Content -Encoding utf8 "$w\a.md"'
Write-Host "  & `"$P\bin\shiori_save.exe`" `"`$w\a.md`""
Write-Host "  (すると `"$S\library\20-projects\shiori\resource\a.md`" ができる)"
Write-Host ''
Write-Host "  詳しい手順と、確かめること: docs\ver7-phase2-3-runbook.md"
