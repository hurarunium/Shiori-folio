# Ver7.0 サーバー: アプリの目視テスト(T5)用に、試験環境(make-test-env.ps1 で作ったもの)へ、ダミーの中身を足す。
# 足すもの: タスク、棚の記事、memory(やること・申し送り)、日誌の申し送り、保留(タグ・プロジェクト・受信箱)、規約・スキル。
# 本物のSSD・libraryには触れない。試験環境のサーバーを止めたままでも、起動したままでも使える。
# 使い方: powershell -ExecutionPolicy Bypass -File scripts\server\seed-app-test.ps1
param([string]$Root = 'C:\shiori-server-test')
$ErrorActionPreference = 'Stop'
$S = Join-Path $Root 'server'
if (-not (Test-Path "$S\library\_system")) { throw "試験環境が無い: $Root(先に make-test-env.ps1)" }
$utf8 = New-Object System.Text.UTF8Encoding($false)   # BOMなし
function Put($path, $text) { New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null; [IO.File]::WriteAllText($path, $text, $utf8) }
$L = "$S\library"

# memory(ホームの「伝言」になる。updated が新しいものが選ばれる)
Put "$L\20-projects\shiori\memory.md" @"
---
title: 詩織の共通記憶(試験)
type: project
kind: memory
project: shiori
updated: 2026-11-22T17:00:00+09:00
---

## 概要

- 試験用の記憶です。

## やること

- [ ] アプリの目視テストを通す。(11/22)
- [ ] 余りのPCへ配置する。(11/23)

## 申し送り・待ち

- Macからの確認待ち。(11/22)
"@
Put "$L\10-common\memory-global.md" "---`ntitle: 全体のmemory(試験)`nkind: memory`nupdated: 2026-11-22T16:00:00+09:00`n---`n`n## 概要`n`n- 試験用です。`n"

# 日誌(申し送りの節)
Put "$L\20-projects\shiori\journal\2026-11-22.md" "---`ntitle: 日誌(試験)`nkind: journal`nproject: shiori`ndate: 2026-11-22`n---`n`n## 14:30 win・claude-code`n`n実装した。`n`n### 申し送り`n`n- 日誌から出る申し送り(memoryが無いときだけ使われる)`n"

# 保留: タグ・プロジェクト(台帳に pending を足す)
Put "$L\_system\tags.yaml" "# タグ(試験用)`ntags:`n  - canonical: 詩織`n    aliases: [shiori]`n    status: confirmed`n  - canonical: 試験タグA`n    status: pending`n  - canonical: 試験タグB`n    status: pending`n"
Put "$L\_system\projects.yaml" "# 台帳(試験用)`nprojects:`n  - id: shiori`n    status: confirmed`n  - id: common`n    status: confirmed`n  - id: trial-project`n    status: pending`n"
# 保留: 受信箱の記事
Put "$L\00-inbox\inbox-a.md" "---`ntitle: 受信箱の記事A(試験)`nreason: tagsが空です`n---`n`n本文A`n"
Put "$L\00-inbox\inbox-b.md" "---`ntitle: 受信箱の記事B(試験)`nreason: projectが未登録です`n---`n`n本文B`n"

# 規約・スキル(規約画面に出る)
Put "$L\_system\CLAUDE.md" "# 保存の規約(試験用)`n`n本文です。`n"
Put "$L\30-resources\skills\work-start.md" "# work-start(試験用)`n`n手順です。`n"

# 知識の棚
Put "$S\knowledge\trial-note.md" "---`ntitle: 試験の棚の記事`ntopic: 試験`nlearned: '2026-11-22'`nsummary: 棚の画面に出る記事です。`nsource: 試験`n---`n`n棚の本文です。`n"

# タスク(shiori_task 経由。サーバーが動いている必要がある。つながらなければ、終了コードが0以外になる)
$task = Join-Path $Root 'portable\bin\shiori_task.exe'
& $task folder add 試験 *> $null
$ok = ($LASTEXITCODE -eq 0)
if ($ok) { & $task add --summary 'アプリのタスク画面を確かめる' --folder 試験 --due 2026-11-25 *> $null; $ok = ($LASTEXITCODE -eq 0) }
if ($ok) { & $task add --summary '期限なしのタスク' --folder 試験 *> $null; $ok = ($LASTEXITCODE -eq 0) }
if ($ok) { Write-Host 'タスクを2件足した。' }
else { Write-Host 'タスクは足せなかった(試験環境のサーバーを起動してから、もう一度このスクリプトを動かすと足せる。他の中身は足してある)。' }
Write-Host "足した: $Root"
Write-Host '次: docs\ver7-phase2-3-runbook.md の T5(アプリの目視テスト)'
