# Ver7.0 サーバー: 窓口(shiori-server)を起動する。
# 例: .\start-server.ps1                          (TailscaleのアドレスのPort 8090で待ち受ける)
#     .\start-server.ps1 -Bind 127.0.0.1:8090     (手元だけで試すとき)
# 0.0.0.0 では待ち受けない。Tailscaleのアドレスがまだ無い(起動直後)ときは、失敗して終わる(1分おきの自動起動がやり直す)。
param(
    [string]$Root = 'C:\ShioriServer',
    [string]$Bind = '',
    [int]$Port = 8090
)
$ErrorActionPreference = 'Stop'
if (-not $Bind) {
    $ip = (& tailscale ip -4 | Select-Object -First 1)
    if (-not $ip) { throw 'Tailscaleのアドレスを取れない(Tailscaleは起動していますか)' }
    $Bind = "$($ip.Trim()):$Port"
}
$config = Join-Path $Root 'server.json'
if (-not (Test-Path $config)) { throw "設定が無い: $config(scripts\server\server.example.json を写して直す)" }
& (Join-Path $Root 'bin\shiori-server.exe') serve --config $config --bind $Bind
