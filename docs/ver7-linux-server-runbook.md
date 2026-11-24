# サーバーを Linux(Ubuntu Server)へ移す手順(試行 → 本番)

2026-11-24 作成。目的: 余りのPC(Windows)のサーバーを、Ubuntu Server 24.04 LTS に移す。
方針: **まず試す。行けそうなら移行する。** Windowsは、試行のあいだ、消さずに本番のまま動かす(戻れる形)。

## 打ち切りの目安
- 今日の夕方(18時ごろ)までに、手順L1〜L7(`rag: true` まで)が通らなければ、Windowsのまま続ける(ゲームジャムを優先)。
- 本番の切り替え(L9)は、L1〜L8が通り、ふらるが決めたときだけ行う。

## コードの状況(11/24に確認)
- `services/shiori-server` は、Linuxを想定済み(`children.rs` の `os_dir()` が `linux`、`pid_alive` は `ps`)。Windows固有は `keep_awake.rs`(Linuxでは何もしない)と、`tasklist`(Windows側の分岐)だけ。
- RAG(`shiori-rag`)は Rust(candle)。Pythonは要らない。
- **直す点**: `children.rs` は、`libs-llama` を `PATH` に足す(Windows向け)。Linuxでは共有ライブラリを `LD_LIBRARY_PATH` で探すため、systemdのユニットで `LD_LIBRARY_PATH` を渡す(`scripts/server/linux/shiori-server.service`)。

## 手順

### L0 準備(ふらる)
- Ubuntu Server 24.04 LTS の ISO と、起動用USB(8GB以上)。
- Linuxを入れる先(Windowsのディスクを消さない): USB接続のSSD、または空いているドライブ。
- 有線LANが使えるなら、有線を使う(Wi-Fiのドライバの問題を避ける)。

### L1 ハードの確認(ライブUSB)
- USBで起動して「Try」の状態で、ネットワーク(有線・Wi-Fi)・ストレージ・画面が認識されるか見る。ここで駄目なら中止。

### L2 インストール
- 先のディスクだけにインストールする(**ディスクの選択を間違えない**。Windowsのディスクには触らない)。
- ユーザーは `shiori`(サービス用)と、操作用の自分のユーザー。OpenSSH サーバーを入れる。
- 入れたら: `sudo timedatectl set-timezone Asia/Tokyo`(世代の05:00と、日誌の日の区切りは、サーバーの時計で決まる)。

### L3 Tailscale
```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up --ssh
```
- 新しい端末として登録される(IPは、Windowsの `100.105.22.70` とは別)。以降は `ssh <ユーザー>@<新しいIP>` で、開発機から直接操作できる。

### L4 眠らせない
```sh
sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target
```
- ふたを閉じても眠らないように、`/etc/systemd/logind.conf` の `HandleLidSwitch=ignore`・`HandleLidSwitchExternalPower=ignore` を設定して、`sudo systemctl restart systemd-logind`。

### L5 ビルド(Linux上で)
```sh
sudo apt install -y build-essential pkg-config git curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/hurarunium/Shiori-folio.git && cd Shiori-folio
(cd services/shiori-server && cargo build --release)
(cd services/shiori-rag && cargo build --release)
```

### L6 配置
```sh
sudo useradd -r -m shiori
sudo mkdir -p /opt/shiori-server/{bin,scripts,runtime/bin/linux,runtime/models}
sudo cp services/shiori-server/target/release/shiori-server /opt/shiori-server/bin/
sudo cp services/shiori-rag/target/release/shiori-rag /opt/shiori-server/runtime/bin/linux/
sudo cp scripts/server/linux/start-server.sh /opt/shiori-server/scripts/ && sudo chmod +x /opt/shiori-server/scripts/start-server.sh
sudo cp scripts/server/linux/server.example.json /opt/shiori-server/server.json
sudo cp scripts/server/linux/shiori-server.service /etc/systemd/system/
```
- `llama-server`(Linux x64・CPU版)を、llama.cpp の公式リリースから取り、`runtime/bin/linux/` に置く(共有ライブラリも同じ場所か `libs-llama/` へ)。**ダウンロードは、ふらるに確認してから**。
- モデル(`runtime/models/embed/nomic-embed-text.gguf` と、リランカーのモデル)は、Windowsのサーバー(`C:\ShioriServer\runtime\models\`)から写す。
- `sudo chown -R shiori:shiori /opt/shiori-server`

### L7 データの写し(Windowsは本番のまま)
- Windowsの `C:\ShioriServer\data\backups\` にある最新の世代zipを、Linuxへ送る(`scp` か `tailscale file cp`)。
- Linux側で復元する(空の場所へ):
```sh
/opt/shiori-server/bin/shiori-server restore <zip> --dest /tmp/restore
sudo mv /tmp/restore/library /opt/shiori-server/library    # knowledge・tasks も同様
```
- `data/`(トークン `tokens.json`)も写す。**トークンを写せば、端末の設定は変わらない**(IPだけ変わる)。
- 索引(`data/rag-index`)は、写さずに作り直す(初回は、Windowsで約1時間かかった)。

### L8 起動と確認
```sh
sudo systemctl daemon-reload
sudo systemctl enable --now shiori-server
journalctl -u shiori-server -f
```
- 確認: 開発機から `/api/v1/status` が `status: ok`・`rag: true`。`read_document`・`search_library` が通る(検索が空でないこと)。
- 試験の宛先は、新しいIP。端末の設定(`data/server.json`)は、まだ変えない。
- 再起動のテスト: `sudo reboot` のあと、人の操作なしで `rag: true` に戻るか。
- 眠りのテスト: 画面オフ(ふたを閉じる)で、一晩、つながり続けるか。

### L9 本番の切り替え(ふらるが決めたとき)
1. 切り替えの直前に、Windowsで新しい世代を作り、L7をやり直す(差分は、RAGが起動時に同期する)。
2. 端末(Windows・Mac)の `server.json` のURLを、新しいIPに変える。MCP・CLI・アプリで確認する。
3. Windowsのサーバーを止める(タスク `ShioriServer` を無効にする)。**Windowsのディスクは、しばらく消さずに残す**(戻れるように)。
4. 数日使って問題が無ければ、古い余りのPCのWindows側を片付ける。

## 戻し方
- Linuxの試行がだめなら、何もせずWindowsのまま。切り替え後に戻すなら、Windowsのタスクを有効にして、端末のURLを `100.105.22.70` に戻し、差分は世代で戻す。

## 残る注意
- 起動時に Tailscale のIPを取るので、`tailscaled` が先に起動している必要がある(ユニットの `After=`)。
- `KillMode=process` により、窓口を止めても子(llama-server・shiori-rag)は残る。完全に止めたいときは、子を別に止める。
