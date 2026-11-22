# Ver7.0 Phase 0: 余りのPCでの試作の手順書

設計書 `design-shiori-ver7-20261118` のPhase 0(試作・関門)の、**余りのPCへの配置と検証4-1〜4-8**の手順。
本物のlibrary・SSDの原本・今の `bin/<os>/` には触れない。使うのは、開発機で作ったダミーのlibraryだけ。

- 余りのPC: `laptop-5h6ft2jk`(Tailscaleのアドレス `100.105.22.70`)
- 窓口の待ち受け: `http://100.105.22.70:9090`(Tailscaleのアドレスだけ。LANには出さない)
- 置き場所(余りのPC): `C:\shiori-proto\`
- 手順の出どころ: `scripts/proto/`(`ver7-proto` ブランチ)

## 0. 全体の流れ

| 段階 | 作業 | 誰が |
|---|---|---|
| 1 | 一式のzipを、開発機から余りのPCへ送る | ふらる |
| 2 | 展開、ファイアウォール、起動の確認 | ふらる |
| 3 | 4-1・4-3・4-8 | ふらる(私が結果を見る) |
| 4 | 4-2(メモリ) | ふらる |
| 5 | 4-4(Claude Code・Codex) | ふらる |
| 6 | 4-5(自動で戻る) | ふらる |
| 7 | 4-6(長時間)・4-7(バックアップ) | 窓口の追加後。この手順書の範囲外 |

結果は、どの段階でも、出力をそのまま貼ってもらえればよい。

## 1. 一式を送る(開発機 → 余りのPC)

zipは作成済み: `C:\shiori-proto\shiori-proto-package.zip`(約886MB。`runtime`・`bin`・`scripts`・`x1`・`x5`・`x10`)。
作り直すときは、開発機で `powershell -File scripts\proto\package-for-spare-pc.ps1`。

**Tailscaleのファイル送信(Taildrop)を使う**(開発機のPowerShell):

```powershell
tailscale file cp "C:\shiori-proto\shiori-proto-package.zip" laptop-5h6ft2jk:
```

**余りのPC**で受け取る(PowerShell。受け取りは、手動で行う):

```powershell
New-Item -ItemType Directory -Force C:\shiori-proto | Out-Null
tailscale file get C:\shiori-proto
```

Taildropが使えない・遅いときは、USBメモリで `shiori-proto-package.zip` を余りのPCの `C:\shiori-proto\` へ置いてもよい。

## 2. 余りのPCでの準備

管理者権限のPowerShellで、順に実行する。

**2-1. 展開**

```powershell
cd C:\shiori-proto
tar -xf shiori-proto-package.zip
dir
```

`runtime`・`bin`・`scripts`・`x1`・`x5`・`x10` が並べばよい。

**2-1b. `llama-server` をCPU版に差し替える(NVIDIAのGPUが無いPCでは必須)**

`runtime` に入っている `llama-server` は、起動時に必ずNVIDIAのドライバ(`nvcuda.dll`)を要求するCUDA版。NVIDIAのGPUが無いPCでは、終了コード `-1073741515` で落ち、窓口が「埋め込みを起動する」を繰り返す(余りのPCはAMDのGPUのみで、これに当たった。11/20)。
開発機で、同じ版(`b10287`、`b06aa774c`)の公式のCPU版(18MB)を試し、埋め込みの結果がCUDA版とほぼ同じ(コサイン類似度 0.9997)ことを確かめてある。

開発機から、差し替え用のzipを送る:

```powershell
tailscale file cp "C:\shiori-proto\patch-cpu.zip" laptop-5h6ft2jk:
```

余りのPCで、zipを `C:\shiori-proto\` へ置いてから(Downloadsに届く場合は移す)、古い物を消して展開する:

```powershell
Remove-Item C:\shiori-proto\runtime\bin\win\libs-llama -Recurse -Force
Remove-Item C:\shiori-proto\runtime\bin\win\llama-server.exe
tar -xf C:\shiori-proto\patch-cpu.zip -C C:\shiori-proto\runtime
dir C:\shiori-proto\runtime\bin\win
```

`llama-server.exe` と `ggml-*.dll` などが、`bin\win` の**直下に並べば**よい(`libs-llama` は無くなる。CPU版は、CPU用のDLLを実行ファイルと同じ場所から探すため)。

**2-2. ファイアウォール(Tailscaleの範囲からの9090番だけを許す)**

```powershell
New-NetFirewallRule -DisplayName ShioriProto -Direction Inbound -Protocol TCP -LocalPort 9090 -RemoteAddress 100.64.0.0/10 -Action Allow
```

**2-3. トークンを2つ発行する**(Windows用とMac用。**表示は1回だけ**なので控える)

```powershell
C:\shiori-proto\bin\shiori-proto-server.exe token-add win --data C:\shiori-proto\data-x1
C:\shiori-proto\bin\shiori-proto-server.exe token-add mac --data C:\shiori-proto\data-x1
```

大きさを変えて試すとき(`x5`・`x10`)は、`--data` の `data-x1` を `data-x5` などに変えて、もう一度発行する。

**2-4. 起動(最初は、手元だけで確認)**

```powershell
C:\shiori-proto\scripts\start-server.ps1 -Size x1 -Bind 127.0.0.1:9090
```

最初の起動は、**索引を作るので、約10分かかる**(ダミー500件、開発機での実測。余りのPCではもっとかかる可能性がある)。
別のPowerShellで、次を実行し、`"embed":true` と `"rag":true` が出るのを待つ:

```powershell
$t = "<winのトークン>"
Invoke-RestMethod http://127.0.0.1:9090/status -Headers @{Authorization="Bearer $t"}
```

**ここで確認してほしいこと:**
- `embed` が `true` にならない場合は、`C:\shiori-proto\data-x1\logs\embed.log` の最後を貼る。余りのPCにGPUが無く、CUDAのDLLが原因の場合は、CPU専用の `llama-server` が必要になる(私が用意する)。
- 起動直後に何か止まったら、窓口のウィンドウの出力を貼る。

準備ができたら、窓口を止めて(`Ctrl+C`)、Tailscaleのアドレスで起動し直す:

```powershell
C:\shiori-proto\scripts\start-server.ps1 -Size x1
```

(子の `llama-server`・`shiori-rag` は残り、次の起動でそのまま使われる。索引は作り直さない。)

## 3. 4-1 つながり方(記事の読み出し時間)

合格の基準(仮): 中継(DERP)でも、記事1件の読み出しが1秒以内。

**Windowsの端末(開発機)**:

```powershell
C:\shiori-proto\scripts\measure-read.ps1 -Url http://100.105.22.70:9090 -Token <winのトークン> -Count 20
```

**Mac**(トークンは、Mac用):

```sh
T=<macのトークン>
for i in $(seq 1 20); do curl -s -o /dev/null -w "%{time_total}\n" -H "Authorization: Bearer $T" "http://100.105.22.70:9090/doc?path=10-projects/proj-alpha/memory.md"; sleep 0.3; done | sort -n | awk '{a[NR]=$1} END{print "最小",a[1],"中央値",a[int(NR/2)+1],"最大",a[NR]}'
```

次の2つの条件で測る(それぞれの結果を貼る):
1. 同じLANにいるとき。
2. 別のネットワーク(スマホのテザリングなど)につないだとき。Tailscale側が「直接」か「中継」かも、`tailscale ping 100.105.22.70`(`via` の部分)で一緒に控える。

## 4. 4-3 衝突検知

**Mac**(ファイルを作る。日本語はUTF-8のファイルで送る):

```sh
T=<macのトークン>; U=http://100.105.22.70:9090
printf '{"path":"00-inbox/conflict.md","content":"最初","expect_rev":"new"}' > c0.json
curl -s -X PUT -H "Authorization: Bearer $T" --data-binary @c0.json $U/doc      # {"rev":"…"} が返る。この rev を控える
```

控えた `rev`(`R`)を、Windowsと Macの**両方**に伝える。次に、できるだけ同時に(声を掛け合って)、それぞれ別の内容で書く:

**Mac**:

```sh
printf '{"path":"00-inbox/conflict.md","content":"Macが書いた","expect_rev":"R"}' > c1.json
curl -s -w " %{http_code}\n" -X PUT -H "Authorization: Bearer $T" --data-binary @c1.json $U/doc
```

**Windows**(PowerShell、`R` と `<winのトークン>` を置き換える):

```powershell
$b = '{"path":"00-inbox/conflict.md","content":"Windowsが書いた","expect_rev":"R"}'
try { Invoke-RestMethod http://100.105.22.70:9090/doc -Method Put -Headers @{Authorization="Bearer <winのトークン>"} -Body ([Text.Encoding]::UTF8.GetBytes($b)) -ContentType 'application/json' } catch { $_.Exception.Response.StatusCode.value__ }
```

合格の基準: **片方が200、もう片方が409**。別の記事(`00-inbox/a-win.md` と `00-inbox/a-mac.md`、どちらも `expect_rev` は `"new"`)を同時に書くと、**両方が200**。

## 5. 4-8 認証の拒否

どれも、200ではなく、拒否されることを確かめる(Windowsの端末から):

```powershell
$u = "http://100.105.22.70:9090/doc?path=10-projects/proj-alpha/memory.md"
# トークンなし
try { Invoke-RestMethod $u } catch { $_.Exception.Response.StatusCode.value__ }
# 誤ったトークン
try { Invoke-RestMethod $u -Headers @{Authorization="Bearer wrong"} } catch { $_.Exception.Response.StatusCode.value__ }
```

失効したトークン(余りのPCで、`mac` を失効させてから、Macで読む):

```powershell
C:\shiori-proto\bin\shiori-proto-server.exe token-revoke mac --data C:\shiori-proto\data-x1
```

Macで同じ `curl` を実行し、401になることを確かめる。確かめたら、`token-add mac2 ...` で新しいトークンを作る(`mac` は作り直せない)。

Tailscaleの外(同じLANの別の機械)から: LANの別の機械で、`http://<余りのPCのLANのアドレス>:9090/health` を開き、**つながらない**ことを確かめる(窓口はTailscaleのアドレスだけで待っているので、つながらないのが正しい)。

## 6. 4-2 8GBでの負荷

メモリを10秒おきに記録する(余りのPCで)。大きさごとに、窓口を起動し直して測る。

**何もしないとき(10分)**:

```powershell
C:\shiori-proto\scripts\measure-memory.ps1 -Out C:\shiori-proto\mem-x1-idle.csv -Minutes 10
```

**検索を繰り返すとき**(別のPowerShellで、同時に):

```powershell
$h = @{Authorization="Bearer <winのトークン>"}
$q = [Text.Encoding]::UTF8.GetBytes('{"query":"観測記録の原因","limit":10,"offset":0}')
1..60 | ForEach-Object { $sw=[Diagnostics.Stopwatch]::StartNew(); Invoke-RestMethod http://100.105.22.70:9090/search_library -Method Post -Headers $h -Body $q -ContentType 'application/json' | Out-Null; "{0:N1}秒" -f $sw.Elapsed.TotalSeconds }
```

`x5`(約2,500件)・`x10`(約5,000件)も同じ手順で測る(`start-server.ps1 -Size x5`、`data-x5` のトークンを使う)。索引の作成は、`x1` で約10分、`x10` ではその約10倍かかる見込みなので、**索引ができるまでの時間も控える**(`status` が `rag:true` になるまで)。

合格の基準(仮): 空きメモリが常に1GB以上。検索が数秒以内。落ちない。

## 7. 4-4 Claude Code・Codexからの接続

中継 `mcp_relay` を、各端末に置く。

- **Windows**: `C:\shiori-proto\bin\mcp_relay.exe`(開発機にも同じものが `services\shiori-proto-server\target\release\` にある)。
- **Mac**: Macでビルドする。`ver7-proto` をpushしたあとで、Macで `git fetch && git switch ver7-proto && cd services/shiori-proto-server && cargo build --release --bin mcp_relay`(出力は `target/release/mcp_relay`)。

登録(Claude Code。Windowsの例):

```powershell
claude mcp add shiori-proto --env SHIORI_SERVER_URL=http://100.105.22.70:9090 --env SHIORI_TOKEN=<winのトークン> -- C:\shiori-proto\bin\mcp_relay.exe
```

Codexは `~/.codex/config.toml` に追加する:

```toml
[mcp_servers.shiori-proto]
command = "C:\\shiori-proto\\bin\\mcp_relay.exe"
env = { SHIORI_SERVER_URL = "http://100.105.22.70:9090", SHIORI_TOKEN = "<winのトークン>" }
```

確かめる: 新しいセッションで、「`shiori-proto` の `search_library` で『観測記録の原因』を検索して」と頼み、続けて、返った `path` を `read_document` で読んでもらう。**4つの組み合わせ**(WindowsのClaude Code・Codex、MacのClaude Code・Codex)で、検索と読み出しが通ることが合格。Codexは、サンドボックスの中からも通るか見る(通らなければ、その旨を貼る)。

> 試作の登録は、確かめたあとで外す(`claude mcp remove shiori-proto`、`config.toml` の項目を削除)。普段使う詩織のMCPとは、別の名前にしてある。

## 8. 4-5 自動で戻る

窓口を、ログインしなくても起動時に始まるタスクとして登録する(余りのPC、管理者権限):

```powershell
C:\shiori-proto\scripts\register-task.ps1 -Size x1
Start-ScheduledTask ShioriProtoServer
```

試す項目(1つずつ。毎回、別の端末から `status` を呼んで確かめる):

| # | 操作 | 期待 |
|---|---|---|
| a | 窓口のプロセスを強制終了(`taskkill /F /IM shiori-proto-server.exe`) | 1〜2分で、窓口が戻る。子は残る |
| b | 子を強制終了(`taskkill /F /IM shiori-rag.exe`、次に `llama-server.exe`) | 数十秒〜数分で戻る(RAGは起動時に同期) |
| c | スリープさせ、復帰させる | 復帰後、つながる |
| d | 再起動して、**ログインしない** | 数分後、別の端末からつながる |
| e | Wi-Fiを切って、つなぐ | 戻ったあと、つながる |

終わったら、タスクを外す: `C:\shiori-proto\scripts\register-task.ps1 -Remove`

## 9. この手順書の外(あとで足す)

- 4-6 長時間(24〜72時間): 呼び出し役のスクリプトを、サーバーの上で動かす。窓口にメモリ・応答の時間のログを足してから。
- 4-7 バックアップ: 世代のzip(manifest つき)の作成と復元。窓口に、世代を作る処理を足してから。

## 10. 後片付け

- 試作の登録を外す(7章、8章)。
- 窓口と子を止める: `taskkill /F /IM shiori-proto-server.exe`、`shiori-rag.exe`、`llama-server.exe`。
- ファイアウォールの規則を消す: `Remove-NetFirewallRule -DisplayName ShioriProto`。
- `C:\shiori-proto\` は、試作が終わるまで残す。
