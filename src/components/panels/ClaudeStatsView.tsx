import { useEffect, useMemo, useState } from "react";
import { api } from "../../api/tauri";
import type { ClaudeStats, ClaudeStatsCounting, ClaudeStatsRange, ClaudeTokens } from "../../types";
import { buildHeatmap, formatDateTime, formatTokens, heatLevel } from "./statsFormat";

const RANGE_LABEL: Record<ClaudeStatsRange, string> = { all: "すべて", days30: "30日", days7: "7日" };
const COUNTING_LABEL: Record<ClaudeStatsCounting, string> = { actual: "実際", app: "アプリ式" };

function totalOf(t: ClaudeTokens): number {
  return t.input + t.cacheCreation + t.cacheRead + t.output;
}

// Claudeモニターの「利用統計」タブ(詩織Ver3.9)。各端末のClaude Codeの履歴から
// 詩織が作った台帳(data/claude-stats/)を合算して表示する。タブを開いたときに、
// このPCの履歴の増えた分を取り込む(前回から間もなければ省く)。
function ClaudeStatsView({ active }: { active: boolean }) {
  const [range, setRange] = useState<ClaudeStatsRange>("all");
  const [counting, setCounting] = useState<ClaudeStatsCounting>("actual");
  const [host, setHost] = useState<string | null>(null);
  const [stats, setStats] = useState<ClaudeStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // 取り込みが終わるたびに増やし、集計を読み直すきっかけにする。
  const [refreshCount, setRefreshCount] = useState(0);

  const refresh = (force: boolean) => {
    setRefreshing(true);
    api
      .refreshClaudeStats(force)
      .then(() => setRefreshError(null))
      .catch((e) => setRefreshError(String(e)))
      .finally(() => {
        setRefreshing(false);
        setRefreshCount((n) => n + 1);
      });
  };

  useEffect(() => {
    if (active) refresh(false);
  }, [active]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    api
      .getClaudeStats(range, host)
      .then((s) => {
        if (cancelled) return;
        setStats(s);
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [active, range, host, refreshCount]);

  const heatmap = useMemo(
    () => (stats ? buildHeatmap(stats.since, stats.until, new Map(stats.daily.map((d) => [d.date, d[counting]]))) : []),
    [stats, counting],
  );
  const maxDaily = useMemo(
    () => Math.max(0, ...heatmap.flat().filter((c) => c.inRange).map((c) => c.value)),
    [heatmap],
  );

  if (error) return <p className="claude-monitor__error">集計に失敗しました: {error}</p>;
  if (!stats) return <p className="claude-monitor__empty">集計しています…</p>;

  const other: ClaudeStatsCounting = counting === "actual" ? "app" : "actual";
  const messages = { actual: stats.messages, app: stats.messagesApp };
  const tokens = { actual: totalOf(stats.actual), app: totalOf(stats.app) };
  const maxModel = Math.max(0, ...stats.models.map((m) => m[counting]));

  return (
    <div className="claude-stats">
      <div className="claude-stats__toolbar">
        <div className="claude-stats__segment" role="group" aria-label="期間">
          {(Object.keys(RANGE_LABEL) as ClaudeStatsRange[]).map((r) => (
            <button
              key={r}
              type="button"
              aria-pressed={range === r}
              className={`claude-stats__chip${range === r ? " claude-stats__chip--active" : ""}`}
              onClick={() => setRange(r)}
            >
              {RANGE_LABEL[r]}
            </button>
          ))}
        </div>
        <div className="claude-stats__segment" role="group" aria-label="数え方">
          {(Object.keys(COUNTING_LABEL) as ClaudeStatsCounting[]).map((c) => (
            <button
              key={c}
              type="button"
              aria-pressed={counting === c}
              className={`claude-stats__chip${counting === c ? " claude-stats__chip--active" : ""}`}
              onClick={() => setCounting(c)}
            >
              {COUNTING_LABEL[c]}
            </button>
          ))}
        </div>
        <select
          className="claude-stats__host"
          aria-label="端末"
          value={host ?? ""}
          onChange={(e) => setHost(e.target.value || null)}
        >
          <option value="">全端末</option>
          {stats.hosts.map((h) => (
            <option key={h.host} value={h.host}>
              {h.host}
            </option>
          ))}
        </select>
      </div>

      {refreshError && <p className="claude-monitor__error">取り込みに失敗しました: {refreshError}</p>}
      {stats.warnings.map((w) => (
        <p key={w} className="claude-monitor__error">{w}</p>
      ))}

      <div className="claude-stats__tiles">
        <div className="claude-stats__tile">
          <span className="claude-stats__label">セッション</span>
          <span className="claude-stats__value">{stats.sessions.toLocaleString()}</span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">メッセージ</span>
          <span className="claude-stats__value">{messages[counting].toLocaleString()}</span>
          <span className="claude-stats__sub">
            {COUNTING_LABEL[other]} {messages[other].toLocaleString()}
          </span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">アクティブ日数</span>
          <span className="claude-stats__value">{stats.activeDays}</span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">合計トークン数</span>
          <span className="claude-stats__value">{formatTokens(tokens[counting])}</span>
          <span className="claude-stats__sub">
            {COUNTING_LABEL[other]} {formatTokens(tokens[other])}
          </span>
        </div>
      </div>

      <section className="claude-stats__section">
        <h3 className="claude-stats__heading">日付別のトークン数</h3>
        <div className="claude-stats__heatmap">
          {heatmap.map((week) => (
            <div key={week[0].date} className="claude-stats__week">
              {week.map((cell) => (
                <span
                  key={cell.date}
                  className={`claude-stats__cell claude-stats__cell--l${cell.inRange ? heatLevel(cell.value, maxDaily) : 0}${
                    cell.inRange ? "" : " claude-stats__cell--outside"
                  }`}
                  title={`${cell.date}: ${formatTokens(cell.value)}トークン`}
                />
              ))}
            </div>
          ))}
        </div>
      </section>

      <section className="claude-stats__section">
        <h3 className="claude-stats__heading">モデル別のトークン数</h3>
        {stats.models.length === 0 && <p className="claude-monitor__empty">この期間の記録はありません。</p>}
        <ul className="claude-stats__models">
          {stats.models.map((m) => (
            <li key={m.model} className="claude-stats__model">
              <span className="claude-stats__model-name">{m.model}</span>
              <span className="claude-stats__bar">
                <span
                  className="claude-stats__bar-fill"
                  style={{ width: `${maxModel > 0 ? (m[counting] / maxModel) * 100 : 0}%` }}
                />
              </span>
              <span className="claude-stats__model-value">{formatTokens(m[counting])}</span>
            </li>
          ))}
        </ul>
      </section>

      <footer className="claude-stats__footer">
        <ul className="claude-stats__hosts">
          {stats.hosts.length === 0 && <li>まだ取り込まれた端末はありません。</li>}
          {stats.hosts.map((h) => (
            <li key={h.host}>
              {h.host}: {formatDateTime(h.updatedAt)}に取り込み
            </li>
          ))}
        </ul>
        <button type="button" className="claude-stats__refresh" disabled={refreshing} onClick={() => refresh(true)}>
          {refreshing ? "取り込み中…" : "今すぐ取り込む"}
        </button>
      </footer>
    </div>
  );
}

export default ClaudeStatsView;
