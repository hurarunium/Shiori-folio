# Ver7.0 Phase 0: 余りのPCで窓口を起動する
# 例: .\start-server.ps1 -Size x1
#     .\start-server.ps1 -Size x5 -Bind 127.0.0.1:9090   (手元だけで試すとき)
# 既定の待ち受けは、このPCのTailscaleのアドレス(tailscale ip -4)の9090番。0.0.0.0 では待ち受けない。
param(
    [ValidateSet('x1', 'x5', 'x10')][string]$Size = 'x1',
    [string]$Bind = ''
)
$ErrorActionPreference = 'Stop'
$root = 'C:\shiori-proto'
if (-not $Bind) {
    $ip = (& tailscale ip -4 | Select-Object -First 1).Trim()
    if (-not $ip) { throw 'Tailscaleのアドレスを取れない(Tailscaleは起動していますか)' }
    $Bind = "${ip}:9090"
}
& "$root\bin\shiori-proto-server.exe" serve `
    --bind $Bind `
    --library "$root\$Size\library" `
    --runtime "$root\runtime" `
    --data "$root\data-$Size"
