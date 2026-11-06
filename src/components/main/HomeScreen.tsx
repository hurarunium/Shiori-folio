import { useEffect, useRef, useState } from "react";
import { api, type ServiceStatus } from "../../api/tauri";
import ArticleDetail from "../library/ArticleDetail";
import { getCategoryMeta, stripSourceExtension } from "../../lib/library";
import { dueSoonTasks } from "../../lib/tasks";
import { useMainViewStore } from "../../store/useMainViewStore";
import { useShioriStore } from "../../store/useShioriStore";
import type { Handoff, LibraryFile, PendingItems, Task } from "../../types";
import DueSlip from "./DueSlip";
import ReceptionShiori from "./ReceptionShiori";
import "./HomeScreen.css";

// 最近更新された記録の上限。実際は画面の高さに入る分だけ出す(スクロールさせないため)。
const RECENT_MAX = 7;
// 記録1行ぶんの高さ(px)。HomeScreen.cssの.home-screen__recent-itemと合わせる。
const RECENT_ROW_HEIGHT = 34;
// 伝言メモ(memory由来の申し送り)に出す項目数。全文は、メモを押してmemoryを開いて読む。
const HANDOFF_TODOS_MAX = 2;
const HANDOFF_NOTES_MAX = 1;

interface Props {
  pending: PendingItems | null;
  onOpenPending: () => void;
}

interface OpenedArticle {
  source: string;
  sourceCategory: string;
  assetPath: string;
}

function greeting(hour: number): string {
  if (hour >= 5 && hour < 10) return "おはようございます";
  if (hour >= 10 && hour < 18) return "こんにちは";
  return "こんばんは";
}

function relativeDay(mtimeSec: number, now: Date): string {
  const day = new Date(mtimeSec * 1000);
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((startOf(now) - startOf(day)) / 86_400_000);
  if (diff <= 0) return "今日";
  if (diff === 1) return "昨日";
  if (diff < 30) return `${diff}日前`;
  return `${day.getMonth() + 1}/${day.getDate()}`;
}

function todayYmd(now: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

// memoryの更新日時(ISO8601)を、伝言メモの隅に書く「10/26」の形にする。
function shortDate(iso: string): string {
  const m = /^\d{4}-(\d{2})-(\d{2})/.exec(iso);
  return m ? `${Number(m[1])}/${Number(m[2])}` : iso;
}

function formatTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(n);
}

// ホーム画面(詩織Ver3.9)。詩織の玄関口として、状態・検索・前回の申し送り・最近の記録・
// 小さな状態行を、スクロールなしの1画面に収める。2026-10-26に図書館の受付の形にした
// (カウンターの奥で詩織が本を読み、検索は紙、申し送りは伝言メモ、最近の記録は新着の棚)。中身はタブを開くたびに読み直す
// (他のアプリやClaudeが書き足すため)。
function HomeScreen({ pending, onOpenPending }: Props) {
  const active = useMainViewStore((s) => s.active === "home");
  const searchInLibrary = useMainViewStore((s) => s.searchInLibrary);
  const setActiveView = useMainViewStore((s) => s.setActive);
  const openTask = useMainViewStore((s) => s.openTask);
  const serviceStatuses = useShioriStore((s) => s.serviceStatuses);

  const [query, setQuery] = useState("");
  const [recent, setRecent] = useState<LibraryFile[] | null>(null);
  const [handoff, setHandoff] = useState<Handoff | null | undefined>(undefined);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [claudeSessions, setClaudeSessions] = useState<number | null>(null);
  const [todayTokens, setTodayTokens] = useState<number | null>(null);
  const [freeGb, setFreeGb] = useState<number | null>(null);
  const [opened, setOpened] = useState<OpenedArticle | null>(null);
  const [recentRows, setRecentRows] = useState(RECENT_MAX);
  const recentRef = useRef<HTMLUListElement>(null);
  const now = new Date();

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const ifCurrent =
      <T,>(apply: (value: T) => void) =>
      (value: T) => {
        if (!cancelled) apply(value);
      };
    // 一覧・申し送りはRAGサーバーから取るため、起動直後はサービスの起動を待つ。
    const backend = useShioriStore.getState().ensureBackendServicesStarted().catch(() => undefined);
    backend
      .then(() => api.listAllKnowledge())
      .then(ifCurrent((files) => setRecent([...files].sort((a, b) => b.mtime - a.mtime).slice(0, RECENT_MAX))))
      .catch(ifCurrent(() => setRecent([])));
    backend
      .then(() => api.getLatestHandoff())
      .then(ifCurrent(setHandoff))
      .catch(ifCurrent(() => setHandoff(null)));
    // タスクはファイルを直接読むので、RAGを待たない。
    api
      .listTasks(false)
      .then(ifCurrent(setTasks))
      .catch(() => undefined);
    api
      .listClaudeSessions()
      .then(ifCurrent((s) => setClaudeSessions(s.filter((x) => x.state !== "ended" && x.state !== "stale").length)))
      .catch(() => undefined);
    api
      .getClaudeStats("days7", null)
      .then(ifCurrent((s) => setTodayTokens(s.daily.find((d) => d.date === todayYmd(new Date()))?.actual ?? 0)))
      .catch(() => undefined);
    api
      .getPortableStorage()
      .then(ifCurrent((s) => setFreeGb(s.freeGb)))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [active]);

  // 最近の記録は、画面の高さに入る行数だけ出す。
  useEffect(() => {
    const el = recentRef.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      const rows = Math.floor(entry.contentRect.height / RECENT_ROW_HEIGHT);
      setRecentRows(Math.max(1, Math.min(RECENT_MAX, rows)));
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [opened]);

  if (opened) {
    return (
      <div className="home-screen home-screen--detail">
        <ArticleDetail
          source={opened.source}
          sourceCategory={opened.sourceCategory}
          assetPath={opened.assetPath}
          onBack={() => setOpened(null)}
        />
      </div>
    );
  }

  const llm = serviceStatuses["llm"];
  const conversationOn = !!llm && !llm.skipped;
  const sign = signOf(serviceStatuses["embedding"], serviceStatuses["rag"], llm);
  const dueSoon = dueSoonTasks(tasks, now);
  const pendingTotal = pending
    ? pending.tags.filter((t) => t.status === "pending").length +
      pending.projects.filter((p) => p.status === "pending").length +
      pending.inbox.filter((i) => i.reviewStatus === null).length
    : null;

  return (
    <div className="home-screen">
      <div className="home-screen__scene">
        <div className="home-screen__shelves" aria-hidden="true" />
        <div className="home-screen__plaque" aria-hidden="true">
          受　付<small>詩織の図書館</small>
        </div>
        <div className="home-screen__shiori">
          <ReceptionShiori />
        </div>
        <p className="home-screen__bubble">
          {greeting(now.getHours())}、ふらるさん。
          <small>{handoff ? "伝言があります。" : ""}今日はどの本をお探しですか。</small>
        </p>
      </div>

      <div className="home-screen__counter">
        <form
          className="home-screen__slip"
          onSubmit={(e) => {
            e.preventDefault();
            if (query.trim()) searchInLibrary(query.trim());
          }}
        >
          <input
            type="text"
            aria-label="蔵書を検索(Enterで図書館へ)"
            placeholder="なにをおさがしですか…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </form>
        {conversationOn && (
          <button
            type="button"
            className="home-screen__bell"
            onClick={() => setActiveView("conversation")}
            title="呼び鈴(会話へ)"
            aria-label="会話を開く"
          >
            🔔
          </button>
        )}
        <div className={`home-screen__sign${sign.open ? "" : " home-screen__sign--wait"}`}>
          <b>{sign.open ? "開館中" : "準備中"}</b>
          {sign.detail}
        </div>
      </div>

      <div className={`home-screen__front${handoff || dueSoon.length > 0 ? "" : " home-screen__front--no-memo"}`}>
        {(handoff || dueSoon.length > 0) && (
          <div className="home-screen__notes">
            {dueSoon.length > 0 && (
              <DueSlip tasks={dueSoon} now={now} onOpenList={() => setActiveView("tasks")} onOpenTask={openTask} />
            )}
            {handoff && (
              <button
                type="button"
                className="home-screen__memo"
                onClick={() =>
                  setOpened({ source: handoff.source, sourceCategory: handoff.sourceCategory, assetPath: "" })
                }
                title={handoff.title}
              >
                <span className="home-screen__memo-head">
                  伝言メモ
                  <small>
                    {[handoff.project, handoff.fromMemory ? shortDate(handoff.date) : handoff.date]
                      .filter(Boolean)
                      .join(" · ")}
                  </small>
                </span>
                {handoff.fromMemory ? (
                  <>
                    {handoff.todos.slice(0, HANDOFF_TODOS_MAX).map((todo) => (
                      <span key={`t-${todo}`} className="home-screen__memo-line">
                        <em>やること</em>
                        {todo}
                      </span>
                    ))}
                    {handoff.notes.slice(0, HANDOFF_NOTES_MAX).map((note) => (
                      <span key={`n-${note}`} className="home-screen__memo-line">
                        <em>申し送り</em>
                        {note}
                      </span>
                    ))}
                  </>
                ) : (
                  <span className="home-screen__memo-line">{handoff.items.join(" / ")}</span>
                )}
              </button>
            )}
          </div>
        )}

        <section className="home-screen__shelf">
          <h3 className="home-screen__heading">
            新着の棚<small>最近更新された記録</small>
          </h3>
          <ul className="home-screen__recent" ref={recentRef}>
            {recent === null && <li className="home-screen__dim">読み込んでいます…</li>}
            {recent?.length === 0 && <li className="home-screen__dim">記録はまだありません。</li>}
            {recent?.slice(0, recentRows).map((file) => (
              <li key={file.relativePath}>
                <button
                  type="button"
                  className="home-screen__recent-item"
                  onClick={() =>
                    setOpened({ source: file.source, sourceCategory: file.sourceCategory, assetPath: file.assetPath })
                  }
                  title={file.title || stripSourceExtension(file.source)}
                >
                  <i
                    className="home-screen__spine"
                    style={{ background: getCategoryMeta(file.sourceCategory).hex }}
                    aria-hidden="true"
                  />
                  <span className="home-screen__recent-title">{file.title || stripSourceExtension(file.source)}</span>
                  <span className="home-screen__dim">{relativeDay(file.mtime, now)}</span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      </div>

      <footer className="home-screen__status">
        <button
          type="button"
          className={`home-screen__status-item${pendingTotal ? " home-screen__status-item--warn" : ""}`}
          onClick={onOpenPending}
        >
          要確認 {pendingTotal ?? "…"}件
        </button>
        <button type="button" className="home-screen__status-item" onClick={() => setActiveView("claude")}>
          Claude {claudeSessions ?? "…"}セッション稼働中
        </button>
        <span className="home-screen__status-item">
          今日 {todayTokens === null ? "…" : formatTokens(todayTokens)}トークン
        </span>
        <span className="home-screen__status-item">SSD 残り {freeGb === null ? "…" : `${Math.round(freeGb)}GB`}</span>
      </footer>
    </div>
  );
}

// 開館中/準備中の札。検索(埋め込み・RAG)が使えれば開館中。会話は外部AIモードでオフでも開館中とする。
function signOf(
  embedding: ServiceStatus | undefined,
  rag: ServiceStatus | undefined,
  llm: ServiceStatus | undefined,
): { open: boolean; detail: string } {
  if (!embedding || !rag) return { open: false, detail: "検索を準備しています" };
  if (!embedding.healthy || !rag.healthy) return { open: false, detail: "検索が起動できていません" };
  if (!llm) return { open: true, detail: "会話を準備しています" };
  if (llm.skipped) return { open: true, detail: "会話はオフ(外部AIモード)" };
  if (!llm.healthy) return { open: true, detail: "会話が起動できていません" };
  return { open: true, detail: "会話・検索とも使えます" };
}

export default HomeScreen;
