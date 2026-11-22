# Ver7.0 Phase 0 (4-2): 開発機で作った索引を、別のPCで使うために、libraryの更新時刻を索引の記録に合わせる
# RAGは、ファイルの更新時刻が記録と完全に一致しないと「変更あり」と見なして作り直す。
# zipなどで写すと小数点以下が失われるので、索引の記録(.index_state.json)の値を、ファイルへ書き戻す。
# 例: .\fix-mtime.ps1 -Library C:\shiori-proto\x5\library -State C:\shiori-proto\data-x5\rag-index\.index_state.json
param(
    [Parameter(Mandatory)][string]$Library,
    [Parameter(Mandatory)][string]$State
)
$ErrorActionPreference = 'Stop'
# .index_state.json は、1行に「"相対パス": 更新時刻,」の形で書かれている。JSONとして読まず、1行ずつ取り出す
$epoch = [DateTime]::new(1970, 1, 1, 0, 0, 0, [DateTimeKind]::Utc).Ticks
$n = 0; $missing = 0
foreach ($line in [IO.File]::ReadLines($State, [Text.Encoding]::UTF8)) {
    if ($line -notmatch '^\s*"(.+)":\s*([0-9.eE+-]+),?\s*$') { continue }
    $rel = $Matches[1]; $val = [double]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)
    $f = Join-Path $Library $rel   # Windowsでは / のままでも開ける
    if (-not (Test-Path -LiteralPath $f)) { $missing++; continue }
    $ticks = $epoch + [int64][math]::Round($val * 1e7)
    [IO.File]::SetLastWriteTimeUtc($f, [DateTime]::new($ticks, [DateTimeKind]::Utc))
    $n++
}
Write-Host "更新時刻を合わせた: $n 件 / 見つからない: $missing 件"
