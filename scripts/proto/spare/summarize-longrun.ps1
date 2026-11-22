# Ver7.0 Phase 0 (4-6 長時間): long-run.ps1 が残したCSVを集計して、合否の材料を出す
# 余りのPCで実行する(CSVを開発機へ送って実行してもよい)。読むだけで、何も書き換えない。
# 例: .\summarize-longrun.ps1
#     .\summarize-longrun.ps1 -Csv C:\shiori-proto\longrun.csv -OutFile C:\shiori-proto\longrun-summary.txt
param(
    [string]$Csv = 'C:\shiori-proto\longrun.csv',
    [string]$OutFile = '',
    [int]$IntervalSec = 60,
    [int]$WarmupHours = 2
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path $Csv)) { throw "CSVが無い: $Csv" }

$lines = New-Object System.Collections.Generic.List[string]
function Say([string]$s) { $lines.Add($s); Write-Host $s }

# ---- 読み込み ----
$rows = @(Import-Csv -Path $Csv -Encoding UTF8)
if ($rows.Count -eq 0) { throw 'CSVに行が無い' }
foreach ($r in $rows) { $r | Add-Member -NotePropertyName t -NotePropertyValue ([datetime]::Parse($r.time)) -Force }
$rows = @($rows | Sort-Object t)

function Num($r, $name) {
    # 行末の改行の取りこぼしがあっても読めるように、空白を除いてから数にする
    $s = ("" + $r.$name).Trim()
    if ($s -eq '') { return [double]0 }
    return [double]$s
}
function Percentile([double[]]$v, [double]$p) {
    if ($v.Count -eq 0) { return 0 }
    $s = $v | Sort-Object
    $i = [math]::Ceiling($p / 100 * $s.Count) - 1
    if ($i -lt 0) { $i = 0 }
    return $s[$i]
}
function Median([double[]]$v) { Percentile $v 50 }
function Slope([double[]]$x, [double[]]$y) {
    # 最小二乗法の傾き
    $n = $x.Count
    if ($n -lt 2) { return 0 }
    $mx = ($x | Measure-Object -Average).Average
    $my = ($y | Measure-Object -Average).Average
    $num = 0.0; $den = 0.0
    for ($i = 0; $i -lt $n; $i++) { $num += ($x[$i] - $mx) * ($y[$i] - $my); $den += ($x[$i] - $mx) * ($x[$i] - $mx) }
    if ($den -eq 0) { return 0 }
    return $num / $den
}

$first = $rows[0].t
$last = $rows[$rows.Count - 1].t
$spanH = ($last - $first).TotalHours

Say "== 4-6 長時間の集計 =="
Say ("CSV: {0}" -f $Csv)
Say ("期間: {0} から {1}(約{2:N1}時間)" -f $first.ToString('MM/dd HH:mm'), $last.ToString('MM/dd HH:mm'), $spanH)
Say ("行数: {0}(間隔{1}秒なら、期待は約{2:N0}行)" -f $rows.Count, $IntervalSec, ($spanH * 3600 / $IntervalSec))
if ($spanH -lt 23.5) { Say "注意: 24時間に届いていない(まだ測定中か、途中で止まった)" }

# ---- 成功・失敗 ----
$sFail = @($rows | Where-Object { $_.search_ok -ne '1' })
$wFail = @($rows | Where-Object { $_.write_ok -ne '1' })
Say ""
Say "-- 成功・失敗 --"
Say ("検索の失敗: {0} / {1}" -f $sFail.Count, $rows.Count)
Say ("書き込みの失敗: {0} / {1}" -f $wFail.Count, $rows.Count)
$errs = @($rows | Where-Object { $_.error -ne '' })
if ($errs.Count -gt 0) {
    Say "エラーの種類(多い順、先頭5つ):"
    $errs | Group-Object error | Sort-Object Count -Descending | Select-Object -First 5 | ForEach-Object {
        $msg = $_.Name; if ($msg.Length -gt 100) { $msg = $msg.Substring(0, 100) + '...' }
        Say ("  {0}回: {1}" -f $_.Count, $msg)
    }
    Say ("失敗の最初: {0} / 最後: {1}" -f $errs[0].t.ToString('MM/dd HH:mm'), $errs[$errs.Count - 1].t.ToString('MM/dd HH:mm'))
}

# ---- 記録の途切れ(窓口やPCが止まっていた時間) ----
Say ""
Say "-- 記録の途切れ(間隔の3倍より長い空き) --"
$gapLimit = $IntervalSec * 3
$gaps = @()
for ($i = 1; $i -lt $rows.Count; $i++) {
    $d = ($rows[$i].t - $rows[$i - 1].t).TotalSeconds
    if ($d -gt $gapLimit) { $gaps += [pscustomobject]@{ From = $rows[$i - 1].t; Sec = [int]$d } }
}
if ($gaps.Count -eq 0) { Say "なし" }
else {
    Say ("{0}回(合計 約{1:N1}分)" -f $gaps.Count, (($gaps | Measure-Object Sec -Sum).Sum / 60))
    $gaps | Sort-Object Sec -Descending | Select-Object -First 5 | ForEach-Object {
        Say ("  {0} から {1}分" -f $_.From.ToString('MM/dd HH:mm'), [math]::Round($_.Sec / 60, 1))
    }
}

# ---- 応答の時間 ----
Say ""
Say "-- 応答の時間(ミリ秒。成功した呼び出しだけ) --"
function Timing($label, $okName, $msName) {
    $ok = @($rows | Where-Object { $_.$okName -eq '1' })
    if ($ok.Count -eq 0) { Say "${label}: 成功なし"; return }
    $v = [double[]]@($ok | ForEach-Object { [double]$_.$msName })
    Say ("{0}: 中央値 {1:N0} / 95% {2:N0} / 最大 {3:N0}" -f $label, (Median $v), (Percentile $v 95), ($v | Measure-Object -Maximum).Maximum)
    # 最初の3時間と最後の3時間を比べる(だんだん遅くなっていないか)
    $head = @($ok | Where-Object { $_.t -lt $first.AddHours(3) } | ForEach-Object { [double]$_.$msName })
    $tail = @($ok | Where-Object { $_.t -ge $last.AddHours(-3) } | ForEach-Object { [double]$_.$msName })
    if ($head.Count -gt 0 -and $tail.Count -gt 0 -and $spanH -gt 6) {
        Say ("  最初の3時間の中央値 {0:N0} → 最後の3時間の中央値 {1:N0}" -f (Median $head), (Median $tail))
    }
}
Timing '検索' 'search_ok' 'search_ms'
Timing '書き込み' 'write_ok' 'write_ms'

# ---- メモリ ----
Say ""
Say "-- メモリ(MB) --"
$availAll = [double[]]@($rows | ForEach-Object { Num $_ 'avail_mb' })
$lowCount = @($availAll | Where-Object { $_ -lt 1024 }).Count
Say ("使えるメモリ(avail_mb): 最小 {0:N0} / 中央値 {1:N0} / 1GB未満になった回数 {2}" -f ($availAll | Measure-Object -Minimum).Minimum, (Median $availAll), $lowCount)

# 1時間ごとの中央値の表
$buckets = @{}
foreach ($r in $rows) {
    $h = [int][math]::Floor(($r.t - $first).TotalHours)
    if (-not $buckets.ContainsKey($h)) { $buckets[$h] = New-Object System.Collections.Generic.List[object] }
    $buckets[$h].Add($r)
}
$hours = @($buckets.Keys | Sort-Object)
Say ""
Say "時間 | 使える | 窓口 | rag | 埋め込み   (各1時間の中央値)"
$series = @{ avail_mb = @(); server_mb = @(); rag_mb = @(); embed_mb = @() }
$hx = @()
foreach ($h in $hours) {
    $b = $buckets[$h]
    if ($b.Count -lt 10) { continue }   # 少ない時間帯は除く
    $med = @{}
    foreach ($k in 'avail_mb', 'server_mb', 'rag_mb', 'embed_mb') {
        $med[$k] = Median ([double[]]@($b | ForEach-Object { Num $_ $k }))
    }
    Say ("{0,3}h | {1,6:N0} | {2,4:N0} | {3,5:N0} | {4,5:N0}" -f $h, $med['avail_mb'], $med['server_mb'], $med['rag_mb'], $med['embed_mb'])
    $hx += $h
    foreach ($k in @($series.Keys)) { $series[$k] += $med[$k] }
}

# 増え続けているかの目安(最初の WarmupHours 時間は、起動直後の変動として除く)
Say ""
Say "-- 増え続けるか(最初の${WarmupHours}時間を除く。目安の基準は仮) --"
$idx = @(); for ($i = 0; $i -lt $hx.Count; $i++) { if ($hx[$i] -ge $WarmupHours) { $idx += $i } }
if ($idx.Count -lt 4) { Say "時間が足りず、判定できない(落ち着いたあとの時間が4つ未満)" }
else {
    $x = [double[]]@($idx | ForEach-Object { $hx[$_] })
    foreach ($k in 'server_mb', 'rag_mb', 'embed_mb') {
        $y = [double[]]@($idx | ForEach-Object { $series[$k][$_] })
        $sl = Slope $x $y
        $n3 = [math]::Min(3, [math]::Floor($y.Count / 2))
        $early = Median ([double[]]($y[0..($n3 - 1)]))
        $late = Median ([double[]]($y[($y.Count - $n3)..($y.Count - 1)]))
        $grow = if ($early -gt 0) { ($late - $early) / $early * 100 } else { 0 }
        $flag = if ($sl -gt 10 -and $grow -gt 10) { '増加傾向(要確認)' } else { '増え続けてはいない' }
        Say ("{0}: 傾き {1:N1} MB/時、前半の中央値 {2:N0} → 後半 {3:N0}({4:N1}%)  => {5}" -f $k, $sl, $early, $late, $grow, $flag)
    }
}

# ---- 合否の材料 ----
Say ""
Say "-- 合否の材料(基準: 落ちない、メモリが増え続けない) --"
Say ("落ちなかったか: 検索の失敗{0}・書き込みの失敗{1}・途切れ{2}回" -f $sFail.Count, $wFail.Count, $gaps.Count)
Say "メモリが増え続けていないか: 上の「増え続けるか」を見る(rag_mb が中心)"
Say "判断は、この結果をふらると見て決める(自動では合否を出さない)"

if ($OutFile) {
    [IO.File]::WriteAllLines($OutFile, $lines, (New-Object Text.UTF8Encoding($true)))
    Write-Host "書き出した: $OutFile"
}
