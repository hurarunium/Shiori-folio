#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Ver7.0 サーバーの通し試験(WindowsでもMacでも動く。ダミーのデータだけを使い、本物のSSD・libraryには触れない)。

実際の shiori-server を 127.0.0.1 の空きポートで立て、shiori-save・shiori-task・mcp_server を、
data/server.json つきの端末(一時フォルダ)から動かして、結果を確かめる。

使い方(リポジトリの直下で。先に、下の3つをビルドしておく):
    cd services/shiori-server && cargo build
    cd src-tauri && cargo build --bin shiori_save --bin shiori_task --bin mcp_server
    python scripts/server/e2e.py
    python scripts/server/e2e.py --release        (release版を使う)
"""
import argparse, json, os, shutil, socket, subprocess, sys, tempfile, time, urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EXE = ".exe" if os.name == "nt" else ""
results = []

def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(("OK  " if ok else "NG  ") + name + (f"   {detail}" if (detail and not ok) else ""))

def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--release", action="store_true")
    a = ap.parse_args()
    prof = "release" if a.release else "debug"
    server_bin = REPO / "services/shiori-server/target" / prof / f"shiori-server{EXE}"
    save_bin = REPO / "src-tauri/target" / prof / f"shiori_save{EXE}"
    task_bin = REPO / "src-tauri/target" / prof / f"shiori_task{EXE}"
    mcp_bin = REPO / "src-tauri/target" / prof / f"mcp_server{EXE}"
    for b in (server_bin, save_bin, task_bin, mcp_bin):
        if not b.exists():
            sys.exit(f"ビルドされていない: {b}\n(ファイル先頭の手順でビルドしてください)")

    work = Path(tempfile.mkdtemp(prefix="shiori-e2e-"))
    S, P = work / "server", work / "portable"
    for d in (S / "library/_system", S / "data", P / "bin", P / "data", P / "work"):
        d.mkdir(parents=True, exist_ok=True)
    (S / "library/_system/projects.yaml").write_text("# 台帳\nprojects:\n  - id: shiori\n    status: confirmed\n  - id: common\n    status: confirmed\n", encoding="utf-8")
    (S / "library/_system/tags.yaml").write_text("tags:\n  - canonical: 詩織\n    aliases: [shiori]\n    status: confirmed\n", encoding="utf-8")
    (S / "library/_system/tier1-profile.md").write_text("# プロファイル(試験)\n", encoding="utf-8")
    port = free_port()
    cfg = {"bind": f"127.0.0.1:{port}", "library": str(S / "library"), "knowledge": str(S / "knowledge"),
           "tasks": str(S / "tasks"), "data": str(S / "data"), "no_children": True, "layout": "v7"}
    (S / "server.json").write_text(json.dumps(cfg), encoding="utf-8")
    out = subprocess.run([str(server_bin), "token", "add", "win", "--data", str(S / "data")], capture_output=True, text=True, encoding="utf-8").stdout
    token = out.strip().splitlines()[-1]
    (P / "config.json").write_text("{}")
    (P / "data/server.json").write_text(json.dumps({"url": f"http://127.0.0.1:{port}", "token": token}))
    exes = {}
    for key, b in (("save", save_bin), ("task", task_bin), ("mcp", mcp_bin)):
        exes[key] = P / "bin" / b.name
        shutil.copy(b, exes[key])

    server = subprocess.Popen([str(server_bin), "serve", "--config", str(S / "server.json")], stderr=subprocess.DEVNULL)
    try:
        for _ in range(60):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1)
                break
            except Exception:
                time.sleep(0.25)
        else:
            sys.exit("サーバーが起動しない")
        run_all(S, P, exes, token, port)
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except Exception:
            server.kill()
        shutil.rmtree(work, ignore_errors=True)

    ng = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(ng)}/{len(results)} 件OK" + ("" if not ng else f"  NG: {ng}"))
    sys.exit(1 if ng else 0)

def cli(exe, *args):
    r = subprocess.run([str(exe), *map(str, args)], capture_output=True, text=True, encoding="utf-8", errors="replace")
    stdout = r.stdout
    try:
        data = json.loads(stdout)
    except Exception:
        data = None
    return r.returncode, data, (r.stderr + stdout)

def run_all(S, P, exes, token, port):
    W = P / "work"
    def write(name, text):
        p = W / name
        p.write_text(text, encoding="utf-8")
        return p

    # ---- 記事 ----
    art = write("a.md", "---\ntitle: 資料\ntype: project\nkind: resource\nproject: shiori\ntags: [shiori, 新タグ]\n---\n本文\n")
    code, d, _ = cli(exes["save"], art)
    check("記事: V7の置き場へ保存される", code == 0 and d and d["destination"] == "library/20-projects/shiori/resource/a.md", d)
    check("記事: タグの別名が直り、未登録のタグが台帳へ入る", d and d["newly_pending_tags"] == ["新タグ"]
          and "- 詩織" in (S / "library/20-projects/shiori/resource/a.md").read_text(encoding="utf-8"))
    check("記事: 保存後に、手元の原本が消える", not art.exists())
    bad = write("bad.md", "---\ntitle: x\ntype: area\nkind: resource\nproject: shiori\ntags: [a]\n---\nx\n")
    code, _, msg = cli(exes["save"], bad)
    check("記事: type: area は断られ、原本は残る", code != 0 and "area" in msg and bad.exists(), msg[:80])
    jn = write("jn.md", "---\ntitle: x\ntype: project\nkind: journal\nproject: shiori\ntags: [a]\n---\nx\n")
    code, _, msg = cli(exes["save"], jn)
    check("記事: 日誌の通常の保存は断られ、--journalへ案内される", code != 0 and "--journal" in msg, msg[:80])

    # ---- memory ----
    mem = lambda body: f"---\ntitle: shiori memory\ntype: project\nkind: memory\nproject: shiori\ntags: [詩織]\n---\n{body}\n"
    code, d, _ = cli(exes["save"], write("mem.md", mem("記憶1")))
    check("memory: 新規に作れる", code == 0 and d and d["destination"] == "library/20-projects/shiori/memory.md", d)
    updated = d["updated"] if d else ""
    code, _, msg = cli(exes["save"], write("mem.md", mem("記憶2")))
    check("memory: 既存の更新には --expect-updated が要る", code != 0 and "expect-updated" in msg, msg[:80])
    code, _, msg = cli(exes["save"], "--expect-updated", "間違い", write("mem.md", mem("記憶2")))
    check("memory: updatedが違えば競合で断られる", code != 0 and "競合" in msg, msg[:80])
    code, d, _ = cli(exes["save"], "--expect-updated", updated, write("mem.md", mem("記憶2")))
    check("memory: 合っていれば更新でき、旧版が退避される", code == 0 and d and d["history_file"], d)

    # 外した項目は、日誌へ自動で記録される。長さの上限による警告は出ない
    upd = d["updated"] if d else ""
    mem_items = lambda items: f"---\ntitle: shiori memory\ntype: project\nkind: memory\nproject: shiori\ntags: [詩織]\n---\n## やること\n\n{items}"
    code, d, _ = cli(exes["save"], "--expect-updated", upd, write("mem.md", mem_items("- A項目 `a.rs` を直す(11/22)\n- Macで `cargo build` を試して、ビルド時間を測る(11/21)\n")))
    upd = d["updated"] if d else ""
    code, d, out = cli(exes["save"], "--expect-updated", upd, write("mem.md", mem_items("- A項目 `a.rs` を直す(11/22)\n")))
    check("memory: 外した項目が、日誌へ自動で記録される", code == 0 and d and d["removed_items"] == 1 and "日誌に記録しました" in out, out[:160])
    day_files = list((S / "library/20-projects/shiori/journal").glob("*.md"))
    check("memory: 日誌に「memoryから外した項目」の節ができる",
          any("### memoryから外した項目" in f.read_text(encoding="utf-8") and "ビルド時間を測る" in f.read_text(encoding="utf-8") for f in day_files))
    check("memory: 長さの上限の警告は出ない", d and all(not isinstance(w, str) for w in d["warnings"]), d)

    # ---- 棚 ----
    code, d, _ = cli(exes["save"], write("math.md", "---\ntitle: 数学\ntype: knowledge\ntags: [数学]\ntopic: 数学\nlearned: 2026-11-01\nsource: 教科書\n---\n棚\n"))
    check("棚: 知識の棚へ保存される", code == 0 and d and d["destination"] == "knowledge/math.md", d)

    # ---- 素材 ----
    (W / "a.zip").write_bytes(b"PK-asset-bytes")
    meta = write("a.zip.meta", "---\ntitle: 素材\ntype: resource\ntags: [詩織]\n---\n説明\n")
    code, d, _ = cli(exes["save"], meta)
    ok = code == 0 and d and d["asset_destination"] == "library/30-resources/files/a.zip"
    check("素材: 実体と.metaが取り込まれる", ok, d)
    check("素材: 実体の中身が一致し、原本が消える",
          (S / "library/30-resources/files/a.zip").read_bytes() == b"PK-asset-bytes" and not (W / "a.zip").exists())
    (W / "b.zip").write_bytes(b"x")
    code, _, _ = cli(exes["save"], "--copy", write("b.zip.meta", "---\ntitle: 素材\ntype: resource\ntags: [詩織]\n---\n説明\n"))
    check("素材: --copy は原本を残す", code == 0 and (W / "b.zip").exists())

    # ---- 日誌 ----
    j = write("j.md", "昼に実装した。\n")
    code, d, _ = cli(exes["save"], "--journal", "--project", "shiori", "--ai", "claude-code", "--summary", "実装した", j)
    check("日誌: 追記でき、見出しに端末とAIが付く", code == 0 and d and d["heading"].endswith("win・claude-code") and d["toc_added"], d)
    code, d, _ = cli(exes["save"], "--journal", "--project", "shiori", write("j2.md", "夕方\n"))
    text = (S / "library/20-projects/shiori/journal" / (d["day"] + ".md")).read_text(encoding="utf-8") if d else ""
    check("日誌: 2回目は末尾に節が足される", code == 0 and text.count("\n## ") >= 3 and "昼に実装した。" in text and "夕方" in text, text[-120:])
    toc = (S / "library/10-common/journal" / (d["day"] + ".md")).read_text(encoding="utf-8") if d else ""
    check("日誌: 全体の日誌の目次に1行入る", "[shiori] 実装した" in toc and toc.count("[shiori]") == 1, toc[-150:])

    code, d, out = cli(exes["save"], "--journal", "--project", "shiori", write("w.md", "### 次にやること\n\n- 設計を読む\n"))
    check("日誌: 書き方の規則の警告が表示され、保存は止まらない", code == 0 and "警告: [J1]" in out and d and d["warnings"], out[:160])

    # ---- 手順書・規約(memoryと同じ考え方: 読む → 直す → 読んだ版のときだけ保存 → 旧版が退避される)----
    skill = "library/30-resources/skills/work-start.md"
    code, d, out = cli(exes["save"], "--doc", skill, write("skill-new.md", "# v1\n手順書の初版。\n"))
    check("手順書: 新規に作れる", code == 0 and d and d["created"] is True, out[:120])
    work = W / "skill-edit.md"
    code, d, out = cli(exes["save"], "--doc-get", skill, "--out", work)
    rev = d["rev"] if d else ""
    check("手順書: 読むと、作業用のファイルに本文が書かれ、版が表示される", code == 0 and rev and work.read_text(encoding="utf-8") == "# v1\n手順書の初版。\n", out[:120])
    work.write_text("# v2\n手順書の2版。\n", encoding="utf-8")
    code, d, out = cli(exes["save"], "--doc", skill, "--expect-rev", rev, work)
    check("手順書: 読んだ版で上書きでき、旧版が退避される", code == 0 and d and d["history_file"] and not work.exists(), out[:160])
    hist = S / d["history_file"].replace("library/", "library/", 1) if d else S
    check("手順書: 退避の中身が旧版そのまま", hist.is_file() and hist.read_text(encoding="utf-8") == "# v1\n手順書の初版。\n")
    stale = write("skill-stale.md", "# v3\n")
    code, _, msg = cli(exes["save"], "--doc", skill, "--expect-rev", rev, stale)
    check("手順書: 古い版では断られ、読み直しを案内される", code != 0 and "先に書き換えました" in msg and "--doc-get" in msg, msg[:160])
    check("手順書: 断られても、中身は変わらない", (S / skill.replace("library/", "library/", 1)).read_text(encoding="utf-8") == "# v2\n手順書の2版。\n" and stale.exists())
    code, _, msg = cli(exes["save"], "--doc", "library/_system/projects.yaml", "--expect-rev", "0" * 64, write("p.yaml", "x"))
    check("手順書: 台帳(projects.yaml)には書けない", code != 0, msg[:120])
    code, _, msg = cli(exes["save"], "--doc", "library/_system/CLAUDE.md", write("sys.md", "新規約"))
    check("手順書: 規約は、新規には作れない(上書きだけ)", code != 0 and "新規" in msg, msg[:120])

    # ---- タスク ----
    code, d, _ = cli(exes["task"], "folder", "add", "詩織")
    check("タスク: フォルダを足せる", code == 0 and d and d["folders"] == ["詩織"], d)
    detail = write("d.md", "詳細\n")
    code, t, _ = cli(exes["task"], "add", "--summary", "設計を読む", "--folder", "詩織", "--due", "2026-12-15", "--detail-file", detail)
    check("タスク: 追加できる", code == 0 and t and t["summary"] == "設計を読む", t)
    tid = t["id"] if t else ""
    code, d, _ = cli(exes["task"], "list")
    check("タスク: 一覧に出る", code == 0 and d and [x["id"] for x in d["tasks"]] == [tid], d)
    code, t, _ = cli(exes["task"], "done", tid)
    check("タスク: 完了にできる", code == 0 and t and t["archived"], t)
    code, _, msg = cli(exes["task"], "delete", tid)
    check("タスク: --yes が無ければ削除しない", code != 0 and "--yes" in msg, msg[:80])
    code, d, _ = cli(exes["task"], "delete", tid, "--yes")
    check("タスク: --yes で削除できる", code == 0 and d and d["deleted"] == tid, d)

    # ---- MCP(JSON-RPC) ----
    mcp = subprocess.Popen([str(exes["mcp"])], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                           text=True, encoding="utf-8", cwd=str(P / "bin"))
    def rpc(i, method, params=None, notify=False):
        msg = {"jsonrpc": "2.0", "method": method}
        if not notify:
            msg["id"] = i
        if params is not None:
            msg["params"] = params
        mcp.stdin.write(json.dumps(msg) + "\n")
        mcp.stdin.flush()
        return None if notify else json.loads(mcp.stdout.readline())
    try:
        rpc(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}})
        rpc(0, "notifications/initialized", notify=True)
        tools = [x["name"] for x in rpc(2, "tools/list")["result"]["tools"]]
        check("MCP: 道具の一覧に read_document がある", "read_document" in tools and "search_library" in tools, tools)
        def call(i, name, args):
            r = rpc(i, "tools/call", {"name": name, "arguments": args})
            return ("ERROR: " + r["error"]["message"]) if "error" in r else r["result"]["content"][0]["text"]
        body = call(3, "read_document", {"path": "library/20-projects/shiori/resource/a.md"})
        check("MCP: read_document がサーバーの正本を読む", "本文" in body, body[:60])
        check("MCP: 置き場の外のパスは断られる", call(4, "read_document", {"path": "library/../x"}).startswith("ERROR"))
        listing = json.loads(call(9, "list_documents", {"path": "library/20-projects/shiori/journal"}))
        entries = listing["entries"]
        check("MCP: list_documents が、日誌の一覧を新しい順に返す", "list_documents" in tools and len(entries) >= 1
              and entries == sorted(entries, key=lambda e: e["name"], reverse=True), listing)
        newest = call(10, "read_document", {"path": entries[0]["path"]}) if entries else ""
        check("MCP: 一覧のpathを、そのままread_documentに渡して読める", "## " in newest and "昼に実装した。" in newest, newest[:80])
        check("MCP: 無いフォルダの一覧は空になる", json.loads(call(11, "list_documents", {"path": "library/20-projects/無い/journal"}))["entries"] == [])
        check("MCP: get_context_profile が読める", "プロファイル" in call(5, "get_context_profile", {}))
        # RAGは動かしていないので、検索は理由つきの誤りになる(つながらない)
        check("MCP: 検索はRAGが無ければ誤りで返る(中継している)", call(6, "search_library", {"query": "詩織"}).startswith("ERROR"))
    finally:
        mcp.terminate()

    # ---- 認証 ----
    req = urllib.request.Request(f"http://127.0.0.1:{port}/api/v1/status", headers={"Authorization": "Bearer wrong"})
    try:
        urllib.request.urlopen(req, timeout=5)
        check("認証: 違うトークンは断られる", False)
    except urllib.error.HTTPError as e:
        check("認証: 違うトークンは断られる", e.code == 401)
    audit = S / "data/audit"
    lines = sum(len(f.read_text(encoding="utf-8").splitlines()) for f in audit.glob("*.jsonl")) if audit.exists() else 0
    check("監査: 操作が監査ログに残る", lines >= 20, f"{lines}行")

if __name__ == "__main__":
    main()
