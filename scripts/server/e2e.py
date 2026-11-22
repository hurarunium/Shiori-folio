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
    check("日誌: 2回目は末尾に節が足される", code == 0 and text.count("\n## ") == 2 and "昼に実装した。" in text and "夕方" in text, text[-120:])
    toc = (S / "library/10-common/journal" / (d["day"] + ".md")).read_text(encoding="utf-8") if d else ""
    check("日誌: 全体の日誌の目次に1行入る", "[shiori] 実装した" in toc and toc.count("[shiori]") == 1, toc[-150:])

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
