# Ver7.0 Phase 0 (4-2, 4-6): メモリを一定の間隔でCSVに記録する(余りのPCで実行)
# 空きメモリと、窓口・RAG・埋め込みの常駐メモリを記録する。
# 例: .\measure-memory.ps1 -Out C:\shiori-proto\mem-x1-idle.csv -Minutes 10
param(
    [Parameter(Mandatory)][string]$Out,
    [int]$Minutes = 10,
    [int]$IntervalSec = 10
)
$end = (Get-Date).AddMinutes($Minutes)
'time,free_mb,server_mb,rag_mb,embed_mb' | Out-File $Out -Encoding utf8
function Mb($name) {
    $p = Get-Process $name -ErrorAction SilentlyContinue
    if ($p) { [math]::Round(($p | Measure-Object WorkingSet64 -Sum).Sum / 1MB) } else { 0 }
}
while ((Get-Date) -lt $end) {
    $os = Get-CimInstance Win32_OperatingSystem
    $free = [math]::Round($os.FreePhysicalMemory / 1KB)
    "{0},{1},{2},{3},{4}" -f (Get-Date -Format 's'), $free, (Mb 'shiori-proto-server'), (Mb 'shiori-rag'), (Mb 'llama-server') |
        Out-File $Out -Append -Encoding utf8
    Start-Sleep -Seconds $IntervalSec
}
Write-Host "記録した: $Out"
