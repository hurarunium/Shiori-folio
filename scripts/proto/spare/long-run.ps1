# Ver7.0 Phase 0 (4-6 長時間): 窓口を、サーバー自身の上から、一定の間隔で呼び続け、結果とメモリをCSVに残す
# 余りのPCで実行する。宛先は、このPC自身のTailscaleのアドレス(開発機をつけておく必要はない)。
# 1回ごとに、検索を1回と、00-inbox/longrun-probe.md の書き換えを1回行い、応答の時間・失敗・メモリを1行に書く。
# 例: .\long-run.ps1 -Hours 24
param(
    [double]$Hours = 24,
    [int]$IntervalSec = 60,
    [string]$Url = '',
    [string]$TokenFile = 'C:\shiori-proto\load-token.txt',
    [string]$Out = 'C:\shiori-proto\longrun.csv',
    [string]$Query = '観測記録の原因'
)
$ErrorActionPreference = 'Stop'
if (-not $Url) {
    $ip = (& tailscale ip -4 | Select-Object -First 1).Trim()
    if (-not $ip) { throw 'Tailscaleのアドレスを取れない' }
    $Url = "http://${ip}:9090"
}
$Url = $Url.TrimEnd('/')
$h = @{ Authorization = 'Bearer ' + (Get-Content $TokenFile -Raw).Trim() }
$utf8 = New-Object Text.UTF8Encoding($false)
$probe = '00-inbox/longrun-probe.md'

function Append-Row($line) { [IO.File]::AppendAllText($Out, $line + "`r`n", $utf8) }
function Mb($name) {
    $p = Get-Process $name -ErrorAction SilentlyContinue
    if ($p) { [math]::Round(($p | Measure-Object WorkingSet64 -Sum).Sum / 1MB) } else { 0 }
}
function Clean($s) { if ($null -eq $s) { '' } else { ($s -replace '[,\r\n]', ' ').Trim() } }

if (-not (Test-Path $Out)) {
    Append-Row 'time,search_ok,search_ms,write_ok,write_ms,error,avail_mb,server_mb,rag_mb,embed_mb'
}

$searchBody = $utf8.GetBytes((@{ query = $Query; limit = 10; offset = 0 } | ConvertTo-Json -Compress))
$end = (Get-Date).AddHours($Hours)
$n = 0
while ((Get-Date) -lt $end) {
    $t0 = Get-Date
    $err = ''

    # 検索
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try {
        Invoke-RestMethod "$Url/search_library" -Method Post -Headers $h -Body $searchBody `
            -ContentType 'application/json' -TimeoutSec 60 | Out-Null
        $sOk = 1
    } catch { $sOk = 0; $err += 'search:' + (Clean $_.Exception.Message) + ' ' }
    $sMs = [int]$sw.Elapsed.TotalMilliseconds

    # 書き込み(読んで rev を得て、expect_rev つきで書く。記事が無ければ new)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    try {
        $rev = 'new'
        try {
            $doc = Invoke-RestMethod "$Url/doc?path=$([uri]::EscapeDataString($probe))" -Headers $h -TimeoutSec 30
            $rev = $doc.rev
        } catch {
            $code = 0
            if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode }
            if ($code -ne 404) { throw }
        }
        $n++
        $content = "長時間の試験用の記事です。回数=$n 時刻=$($t0.ToString('s'))`n"
        $body = $utf8.GetBytes((@{ path = $probe; content = $content; expect_rev = $rev } | ConvertTo-Json -Compress))
        Invoke-RestMethod "$Url/doc" -Method Put -Headers $h -Body $body -ContentType 'application/json' -TimeoutSec 30 | Out-Null
        $wOk = 1
    } catch { $wOk = 0; $err += 'write:' + (Clean $_.Exception.Message) }
    $wMs = [int]$sw.Elapsed.TotalMilliseconds

    $avail = (Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory).AvailableMBytes
    Append-Row ('{0},{1},{2},{3},{4},{5},{6},{7},{8},{9}' -f $t0.ToString('s'), $sOk, $sMs, $wOk, $wMs, (Clean $err), $avail,
        (Mb 'shiori-proto-server'), (Mb 'shiori-rag'), (Mb 'llama-server'))

    $wait = $IntervalSec - ((Get-Date) - $t0).TotalSeconds
    if ($wait -gt 0) { Start-Sleep -Seconds $wait }
}
Write-Host "終わった: $Out"
