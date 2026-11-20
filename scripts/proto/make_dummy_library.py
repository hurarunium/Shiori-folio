#!/usr/bin/env python3
"""Ver7.0 Phase 0 試作用のダミーlibraryを作る。

- 本物のlibrary(E:\\portable\\library)と同じフォルダ構成で、個人の情報を含まない日本語の記事を作る。
- 本物のlibraryや `bin/<os>/` には一切触れない。出力先は引数で指定し、`E:\\portable` 配下は拒否する。
- 同じ seed なら同じ中身になる(再現できる)。

使い方:
    python scripts/proto/make_dummy_library.py --count 500  --out C:\\shiori-proto\\x1
    python scripts/proto/make_dummy_library.py --count 2500 --out C:\\shiori-proto\\x5
    python scripts/proto/make_dummy_library.py --count 5000 --out C:\\shiori-proto\\x10

出力: <out>/library/ と <out>/tasks/(本物の portable/ と同じ並び)
"""
import argparse
import math
import random
import sys
from pathlib import Path

# ダミーのプロジェクト(実在のプロジェクト名は使わない)。type と記事の割合を持つ。
PROJECTS = [
    ("proj-alpha", "project", 0.14), ("proj-beta", "project", 0.12),
    ("proj-gamma", "project", 0.08), ("proj-delta", "project", 0.06),
    ("area-shiori-dummy", "area", 0.22), ("area-study", "area", 0.14),
    ("area-hobby", "area", 0.10), ("area-health", "area", 0.06),
]

# 検索の確認に使える、プロジェクトごとの固有語(日本語)
TOPIC_WORDS = {
    "proj-alpha": ["星図", "天文台", "望遠鏡", "軌道計算", "観測記録"],
    "proj-beta": ["園芸", "温室", "土壌", "水やり", "収穫"],
    "proj-gamma": ["鉄道", "時刻表", "路線図", "駅舎", "車両基地"],
    "proj-delta": ["料理", "出汁", "発酵", "仕込み", "献立"],
    "area-shiori-dummy": ["索引", "埋め込み", "検索", "蔵書", "司書"],
    "area-study": ["数学", "線形代数", "証明", "演習", "ノート"],
    "area-hobby": ["写真", "現像", "構図", "レンズ", "撮影"],
    "area-health": ["睡眠", "運動", "食事", "体重", "記録"],
}

COMMON = ["確認", "整理", "方針", "手順", "結果", "課題", "改善", "検討", "記録", "予定",
          "進捗", "判断", "原因", "対応", "比較", "調整", "見直し", "共有", "準備", "完了"]

SENT_TEMPLATES = [
    "今日は{a}について{b}を行い、{c}の見通しが立った。",
    "{a}の{b}を見直したところ、{c}との食い違いが見つかった。",
    "次回までに{a}の{b}をそろえ、{c}を確かめる予定にしている。",
    "{a}は思ったより{b}が重く、{c}を先に進めることにした。",
    "{a}と{b}を比べた結果、{c}の方が扱いやすいと判断した。",
    "{b}の記録を読み返すと、{a}の{c}が曖昧なままだった。",
    "{a}に関する{b}は一通り済んだが、{c}はまだ途中である。",
    "メモとして、{a}の{c}は{b}のあとに回す。",
]

KINDS = ["journal", "resource"]


def sizes_for(rng, n):
    """1件ごとの本文の文字数。本物は平均約24KB(11.5MB/485件)。UTF-8の日本語は約3バイト/文字。"""
    out = []
    for _ in range(n):
        v = rng.lognormvariate(math.log(5500), 0.7)  # 文字数の中央値およそ5500字 => 約16KB
        out.append(int(min(max(v, 600), 30000)))
    return out


def sentence(rng, topics):
    t = rng.choice(SENT_TEMPLATES)
    pool = topics + COMMON
    return t.format(a=rng.choice(pool), b=rng.choice(COMMON), c=rng.choice(pool))


def body(rng, topics, chars, marker):
    parts = [f"## 概要\n\nダミー記事 {marker}。試作の検証用に機械で作った文章で、個人の情報は含まない。\n"]
    total = len(parts[0])
    sec = 1
    while total < chars:
        head = f"\n## 項目{sec}: {rng.choice(topics)}の{rng.choice(COMMON)}\n\n"
        para = "".join(sentence(rng, topics) for _ in range(rng.randint(4, 9))) + "\n"
        parts += [head, para]
        total += len(head) + len(para)
        sec += 1
    return "".join(parts)


def frontmatter(d):
    lines = ["---"]
    for k, v in d.items():
        if isinstance(v, list):
            if v:
                lines.append(f"{k}:")
                lines += [f"- {x}" for x in v]
            else:
                lines.append(f"{k}: []")
        else:
            lines.append(f"{k}: {v}")
    lines.append("---\n")
    return "\n".join(lines)


def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--count", type=int, required=True, help="記事の件数(目安: 500 / 2500 / 5000)")
    ap.add_argument("--out", required=True, help="出力先。<out>/library/ と <out>/tasks/ を作る")
    ap.add_argument("--seed", type=int, default=20261120)
    a = ap.parse_args()

    out = Path(a.out).resolve()
    # 本物の実機(SSD)の配下には作らない
    if str(out).lower().replace("/", "\\").startswith("e:\\portable"):
        sys.exit("出力先が本物の portable/ 配下です。中止します。")
    if out.exists() and any(out.iterdir()):
        sys.exit(f"出力先が空ではありません: {out}(上書きしません。空のフォルダを指定してください)")

    rng = random.Random(a.seed)
    lib = out / "library"
    # 固定のファイル: プロジェクトごとのmemory、全体のmemory、_system
    fixed = 0
    for pid, typ, _ in PROJECTS:
        folder = "10-projects" if typ == "project" else "20-areas"
        topics = TOPIC_WORDS[pid]
        fm = frontmatter({
            "title": f"{pid}の共通記憶(ダミー)", "type": typ, "kind": "memory",
            "tags": ["ダミー", "共通記憶"], "project": pid,
            "summary": f"{pid}の、いま有効な状態(ダミー)。", "index": "true", "status": "new",
            "date": "2026-10-10T00:00:00+09:00", "updated": "2026-11-18T00:00:00+09:00",
        })
        write(lib / folder / pid / "memory.md", fm + body(rng, topics, 1500, f"DUMMY-MEM-{pid}"))
        fixed += 1
    write(lib / "40-profile" / "memory-global.md", frontmatter({
        "title": "全体の共通記憶(ダミー)", "type": "profile", "kind": "memory",
        "tags": ["ダミー", "共通記憶"], "summary": "全体用のダミー。", "index": "true",
        "status": "new", "date": "2026-10-10T00:00:00+09:00", "updated": "2026-11-18T00:00:00+09:00",
    }) + body(rng, COMMON, 1500, "DUMMY-MEM-GLOBAL"))
    fixed += 1
    write(lib / "_system" / "projects.yaml",
          "projects:\n" + "".join(f"- id: {p}\n  type: {t}\n" for p, t, _ in PROJECTS))
    write(lib / "_system" / "tags.yaml", "tags:\n- ダミー\n- 共通記憶\n- 運用\n- 検証\n")
    (lib / "00-inbox").mkdir(parents=True, exist_ok=True)

    # 記事本体
    n = max(a.count - fixed, 0)
    weights = [w for _, _, w in PROJECTS]
    chars = sizes_for(rng, n)
    made = 0
    total_bytes = 0
    for i in range(n):
        pid, typ, _ = rng.choices(PROJECTS, weights=weights)[0]
        topics = TOPIC_WORDS[pid]
        kind = rng.choice(KINDS)
        # 古い記録の一部は 90-archive に置く(本物でも約25%がarchive)
        if rng.random() < 0.2:
            base = lib / "90-archive" / ("10-projects" if typ == "project" else "20-areas") / pid
        else:
            base = lib / ("10-projects" if typ == "project" else "20-areas") / pid / kind
        marker = f"DUMMY-{i:05d}"
        title = f"{rng.choice(topics)}の{rng.choice(COMMON)}({marker})"
        day = rng.randint(1, 28)
        mon = rng.randint(8, 11)
        fm = frontmatter({
            "title": title, "type": typ, "kind": kind,
            "tags": ["ダミー", rng.choice(topics)], "project": pid,
            "summary": f"{rng.choice(topics)}と{rng.choice(COMMON)}についてのダミー記事。",
            "index": "true", "status": "new", "related": [],
            "date": f"2026-{mon:02d}-{day:02d}T09:00:00+09:00",
        })
        text = fm + body(rng, topics, chars[i], marker)
        write(base / f"{kind}-{marker.lower()}.md", text)
        made += 1
        total_bytes += len(text.encode("utf-8"))

    # tasks/(本物の portable/tasks/ と同じ並び。数件)
    write(out / "tasks" / "INDEX.md", "# タスクの一覧(自動生成。直接編集しない)\n\n"
          + "".join(f"- task-dummy-{i:03d} | 締切 2026-12-{10+i:02d} | ダミー | タスク{i}\n" for i in range(3)))
    for i in range(3):
        write(out / "tasks" / "todo" / f"task-dummy-{i:03d}.md",
              f"---\nsummary: ダミーのタスク{i}\ncreated: 2026-11-13\ndue: 2026-12-{10+i:02d}\n---\n\n本文{i}。\n")

    print(f"作成: {a.count}件(固定{fixed} + 記事{made})")
    print(f"記事の合計サイズ: {total_bytes/1024/1024:.1f} MB(平均 {total_bytes/max(made,1)/1024:.1f} KB)")
    print(f"出力先: {out}")


if __name__ == "__main__":
    main()
