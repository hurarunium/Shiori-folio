# `_system/CLAUDE.md`(libraryの保存規約)の改訂案(Ver7.0の下書き)

対象: `library/_system/CLAUDE.md`(いま約430行)。移行の日に、下の改訂を入れる。それまでは、v6.2の規約のまま。
行番号は、2026-11-22時点のもの。**新しい文言は、そのまま貼れる形**で示す。

## 1. 前文(3〜8行目)— 直接書き込みの廃止

**旧**: 「Claude CodeやCodexなどのAIは、このディレクトリへ直接frontmatter付きmdを書き込める(MCPツール経由ではない。…)」
**新**:

> Ver7.0以降、libraryの正本は、余りのPCで動くサーバーにある(外付けSSDは、バックアップ専用)。AI(Claude Code・Codexなど)は、libraryへ**直接書き込めない**。読むときは、詩織のMCP(`search_library`・`list_documents`・`read_document`)、書くときは、`shiori-save`(記事・memory・素材・棚)、`shiori-save --journal`(日誌の追記)、`shiori-task`(タスク)を使う。配置先の判断・タグの正規化・検証・衝突の検知は、サーバーが、決定的なルールとして行う。

## 2. frontmatter(12〜24行目、42行目)

- `type: (project | area | resource | profile)` → `type: (project | resource | profile)`(**`area`は廃止**。古い`type: area`は、移行のときに`type: project`へ書き換える。移行後に残っていたら、`shiori-save`が断る)
- `kind: (journal | resource。typeがproject/areaの記事のみ必須。…)` → `kind: (journal | resource。typeがprojectの記事のみ必須。共通記憶MDだけは memory、下記参照。日誌は shiori-save --journal で追記するので、kind: journal の記事を通常の保存では作れない)`
- `project: (typeがproject/areaの場合のみ。10-projects/・20-areas/の各識別子)` → `project: (typeがprojectの場合のみ。20-projects/の識別子。全体の日誌・全体用のmemoryは common)`
- 42行目「`type`がproject/areaなのに`kind`が不正の場合は…」→「projectなのに…」
- 「`title`等の値にコロンを含む場合は、ダブルクォート…AIがこのディレクトリへ直接書き込む場合は手動でのクォートが必要」→ 後半を、「`shiori-save`の保存は、自動で正しくエスケープされる」だけにする(直接書き込みの廃止のため)。

## 3. 配置先の決め方(60〜77行目)

**新**:

1. `type: project`(期限・ゴールのある進行中の取り組みと、終わりのない継続的な関心領域。旧`type: area`を統合) → `project`が`_system/projects.yaml`に存在すれば`20-projects/{project}/{kind}/`。存在しない/空欄なら`00-inbox/`(存在しないprojectは`pending`で自己登録される)。
2. `type: resource`(project/areaに紐づかない、他人から得たことの記録) → `project`があっても無視し、`30-resources/`の直下に置く。
3. `type: profile`(ふらるさん自身についての記録) → `category`が指定されていれば`40-profile/{category}/`、未指定なら`40-profile/`直下。**全体用のmemory(`memory-global.md`)だけは、`10-common/`に置く**(`type: profile`のまま)。
4. 全体の日誌・全体用のmemoryの置き場: `10-common/`(`journal/YYYY-MM-DD.md`、`memory-global.md`)。

## 4. `kind: journal`(81〜87行目)→ 「日誌」の節に置き換える

**旧**の`kind: journal`の項(作業ログ、1作業1ファイル)を、次に置き換える。

> - `kind: journal` → 日誌。**プロジェクトごと・日付ごとに1ファイル**(`20-projects/{project}/journal/YYYY-MM-DD.md`)で、**追記だけ**ができる。通常の保存で作ることはできない(下の「日誌」の節)。

## 5. 新しい節「日誌(Ver7.0)」(「共通記憶MD」の前に足す)

> ## 日誌(`kind: journal`、詩織Ver7.0)
>
> その日にやったことと、起きたことの記録。プロジェクトごと・日付ごとに1ファイル。
>
> - **置き場**: `20-projects/{project}/journal/YYYY-MM-DD.md`。全体の日誌(どのプロジェクトにも属さない出来事)は`10-common/journal/YYYY-MM-DD.md`(`project: common`)。
> - **日の区切りは午前5時**(サーバーの時計。日本時間)。午前0〜5時の作業は、前の日の日誌に入る。
> - **追記**: `shiori-save --journal --project <名前> [--ai <名前>] [--summary "1行"] <節の本文.md>`。節の本文だけを書く(frontmatterも`##`の見出しも付けない。節の中の見出しは`###`から)。サーバーが、日付のファイルを作り(無ければ)、末尾に節を足す。
> - **節の見出し**: サーバーが付ける(`## HH:MM 端末・AI`。午前0〜5時の節は`## MM/DD HH:MM 端末・AI`)。端末名は、トークンの名前(`win`・`mac`など)。
> - **全体の日誌の目次**: `--summary`を付けると、全体の日誌の`## 目次`に`- HH:MM [project] 要約`が1行入る。
> - **書き換え・削除はできない**(追記だけ)。直したいときは、訂正の節を、新しく追記する。
> - **通常の保存では作れない**: `kind: journal`の記事を`shiori-save <md>`で保存すると、サーバーが断る(日付の名前の重複で連番がつく事故を防ぐため)。
> - **書き方の規則と、警告**: `20-areas/shiori/resource/rules-journal-memory-ver7-20261122.md`(移行後は`20-projects/shiori/resource/…`)。保存の結果の`warnings`に、規則の警告が返る(保存は止まらない)。

## 6. frontmatterスキーマの表(116行目)

`kind`の行: 「`journal`/`resource`。typeがproject/areaの記事のみ必須。…」→「`journal`/`resource`。typeがprojectの記事のみ必須(日誌は`--journal`で追記)。共通記憶MDだけは`memory`(下記。typeはproject/profile)」。
`day`・`updated`の行を足す(日誌が持つ): `day`(日誌の日、`YYYY-MM-DD`)、`updated`(memoryに加え、**日誌も持つ**。追記のたびに、サーバーが書く)。

## 7. 共通記憶MD(140〜232行目)

- 置き場所の表(153〜156行目): 

  | 種類 | type | 場所 |
  |---|---|---|
  | プロジェクト単位 | project | `20-projects/{project}/memory.md`(`journal/`・`resource/`の下ではなく直下) |
  | ふらるさんとAI全体 | profile | `10-common/memory-global.md` |

  (領域単位の行は、`type: area`の廃止で削除)
- frontmatterの例(166〜168行目): `type: area` → `type: project`、`# project/areaのとき必須` → `# projectのとき必須`。
- 「保存は`shiori-save`経由のみ」の1.「既存のmemoryをファイルとして読み」→「`read_document`で読み」。
- **「内容の規則」の置き換え**(長さの上限を廃止):
  - 旧「上限は、本文で約200行・6,000字。超えると保存時に警告が出る…」と、旧「各項目は1行で短く(目安は60字前後)…」を、次に置き換える。

    > - **長さの上限は設けない**(字数・行数・1項目の長さ、いずれも)。上限に合わせて削ると、残したい情報が失われるため。保存の結果に、大きさ(`size`)が、情報として返るだけ。詳細が増えたら、別の記事(`resource`など)に置き、memoryからは1行で参照する。
    > - **内容の密度を重視する**。良い1行は、(1)ファイル名・関数名・ID・数値・日付・コマンドのどれかを含み、(2)結論と根拠(または、状態と次の条件)があり、(3)1行に1つの事実で、(4)曖昧語を単独で使わず、(5)別の場所にある内容を再掲しない。規則と警告の条件(M1・D1〜D4など)は、`rules-journal-memory-ver7-20261122`。
    > - 「やること」「申し送り・待ち」の項目には、**末尾に日付 `(11/22)`** を付ける。
    > - **外した項目は、サーバーが、そのプロジェクトの日誌へ自動で記録する**(`### memoryから外した項目`の節。書き換えられた項目は除く。全体用のmemoryは、全体の日誌へ)。整理で、残したい情報が失われないようにするため。
- 旧版の退避(3世代)は、そのまま。外した項目は、期限なく、日誌に残る。
- 「読む・更新するタイミング」: 「SSDが未接続でmemoryが読めないときは…」→「サーバーにつながらないときは、黙って別の動作をせず、その旨をふらるに伝える(手元のSSDや古いコピーを、代わりに読まない)」。
- 「`40-profile/`の例外」: 「`memory-global.md`だけはAIが更新してよい」→「`10-common/memory-global.md`は、AIが(`shiori-save`経由で)更新してよい」。

## 8. 素材(296〜312行目)

置き場所の表: `project | 20-projects/{project}/files/`。**`area`の行は削除**。

## 9. アーカイブ(257〜272行目、369〜377行目)

- 「5分類(`00-inbox`/`10-projects`/`20-areas`/`30-resources`/`40-profile`)」→「`00-inbox`/`10-common`/`20-projects`/`30-resources`/`40-profile`」。
- 例の`10-projects/…` → `20-projects/…`、`90-archive/10-projects/…` → `90-archive/20-projects/…`(**新しいアーカイブは`90-archive/20-projects/{project}/`**)。
- 既存のアーカイブ(`90-archive/10-projects`・`90-archive/20-areas`)は、移行のときに、同じ変換で`90-archive/20-projects/`へ移る。

## 10. 機械的な置き換え(残りの記録のパス)

記録の本文にある、古いパス(`20-areas/shiori/resource/…`・`10-projects/…`)は、移行のときに、ファイルが`20-projects/`へ移るので、**規約・skillの中の参照は、機械的に`20-projects/`へ直す**(147・148・405・418・432行目など)。過去の日誌・企画書・設計書の本文の古いパスは、書き換えない(当時の事実として残す。`related`はファイル名だけなので、リンクは壊れない)。

置き換えの規則: `10-projects/` → `20-projects/` / `20-areas/` → `20-projects/` / `40-profile/memory-global.md` → `10-common/memory-global.md` / `90-archive/10-projects/`・`90-archive/20-areas/` → `90-archive/20-projects/`。

## 11. 未解決の課題(下書きの範囲では、決められないもの)

**skill の本体(`30-resources/skills/`)・`_system/` の資料・libraryの中のスクリプト(`cross-review.sh`)を、どう更新するか。**

- いまの規約は、これらを「ファイルを直接書き込み、索引に入れる(`/reindex_file`)」としている(「配置先の決め方」の3、「skillの本体」の節)。
- Ver7.0では、libraryは端末から見えず、サーバーの窓口には、これらを書く道が無い(`shiori-save`は記事の保存だけ。`PUT /api/v1/doc`は受信箱だけ)。
- **案**: (A) サーバーの管理用コマンド(`shiori-server doc put`)で、余りのPCから書く。(B) `PUT /api/v1/doc`の許可を、`library/30-resources/skills/`・`library/_system/`(ただし`projects.yaml`・`tags.yaml`・`memory-history/`は除く)へ広げ、`expect_rev`つきで、`read_document`で読んだ版から書き換える。(C) 専用のCLI(`shiori-doc`)を足す。
- 推奨は(B)か(C)。skillを、AIが自分で直す運用(`skill改善提案`)が、いまもあるため。**ふらるの判断が要る**。
- `cross-review.sh`: 移行後は、`read_document`で取り出して一時ファイルから動かすか、各端末の`~/.claude/skills/`などへ置き直すかも、あわせて決める。
