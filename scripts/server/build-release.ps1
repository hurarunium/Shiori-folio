# Ver7.0 サーバー: リリース版をビルドして、配置用のフォルダ(dist\ShioriServer)にまとめる。
# 開発機で実行する。できたフォルダを、余りのPCの C:\ShioriServer へ写す(Taildropなど)。
$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Push-Location (Join-Path $repo 'services\shiori-server')
try { cargo build --release; if ($LASTEXITCODE -ne 0) { throw 'ビルドに失敗' } } finally { Pop-Location }
$out = Join-Path $repo 'dist\ShioriServer'
New-Item -ItemType Directory -Force (Join-Path $out 'bin'), (Join-Path $out 'scripts') | Out-Null
Copy-Item (Join-Path $repo 'services\shiori-server\target\release\shiori-server.exe') (Join-Path $out 'bin') -Force
Copy-Item (Join-Path $PSScriptRoot '*.ps1') (Join-Path $out 'scripts') -Force
Copy-Item (Join-Path $PSScriptRoot 'server.example.json') (Join-Path $out 'server.example.json') -Force
Write-Host "できた: $out"
Write-Host '余りのPCでは、server.example.json を server.json として写して直し、runtime(shiori-rag・llama-server・モデル)を置く。'
