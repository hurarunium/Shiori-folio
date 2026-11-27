import { useEffect, useMemo, useState } from "react";
import { api } from "../../api/tauri";
import type { ClaudeStatsRange, CodexLimit, CodexLimitWindow, CodexStats } from "../../types";
import { buildHeatmap, formatDateTime, formatTokens, heatLevel } from "./statsFormat";
import "./ClaudeMonitorPanel.css";

const RANGE_LABEL: Record<ClaudeStatsRange, string> = { all: "すべて", days30: "30日", days7: "7日" };

// 枠の長さ(分)の表示。日や時間で割り切れるときは、その単位で出す。
function windowLabel(minutes: number | null): string {
  if (minutes === null) return "制限の枠";
  if (minutes % 1440 === 0) return `${minutes / 1440}日の枠`;
  if (minutes % 60 === 0) return `${minutes / 60}時間の枠`;
  return `${minutes}分の枠`;
}

function resetText(window: CodexLimitWindow, nowMs: number): string {
  if (window.resetsAt === null) return "リセット時刻は不明";
  const at = formatDateTime(window.resetsAt * 1000);
  // 過ぎていれば、いまの使用率は観測した値と違う。
  return window.resetsAt * 1000 <= nowMs ? `リセット時刻(${at})を過ぎた値です` : `${at}にリセット`;
}

function LimitView({ limit, nowMs }: { limit: CodexLimit; nowMs: number }) {
  const windows = [limit.primary, limit.secondary].filter((w): w is CodexLimitWindow => w !== null);
  return (
    <div className="codex-stats__limit">
      {windows.map((w, i) => (
        <div key={i}>
          <div className="codex-stats__row">
            <span>{windowLabel(w.windowMinutes)}</span>
            <span className="claude-stats__bar">
              <span className="claude-stats__bar-fill" style={{ width: `${Math.min(100, Math.max(0, w.usedPercent))}%` }} />
            </span>
            <span className="claude-stats__model-value">{w.usedPercent}%使用</span>
          </div>
          <p className="codex-stats__note">{resetText(w, nowMs)}</p>
        </div>
      ))}
      <p className="codex-stats__note">
        {limit.limitId}
        {limit.planType && `(${limit.planType})`}: {formatDateTime(limit.observedAt * 1000)}に観測
      </p>
    </div>
  );
}

// AIの画面の「Codex」(詩織Ver7.0.2)。各端末のCodexの履歴から詩織が作った台帳(data/codex-stats/)を
// 合算して表示する。開いたときに、このPCの履歴の増えた分を取り込む(前回から間もなければ省く)。
function CodexStatsView({ active }: { active: boolean }) {
  const [range, setRange] = useState<ClaudeStatsRange>("all");
  const [host, setHost] = useState<string | null>(null);
  const [stats, setStats] = useState<CodexStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // 取り込みが終わるたびに増やし、集計を読み直すきっかけにする。
  const [refreshCount, setRefreshCount] = useState(0);

  const refresh = (force: boolean) => {
    setRefreshing(true);
    api
      .refreshCodexStats(force)
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
      .getCodexStats(range, host)
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
    () => (stats ? buildHeatmap(stats.since, stats.until, new Map(stats.daily.map((d) => [d.date, d.total]))) : []),
    [stats],
  );
  const maxDaily = useMemo(
    () => Math.max(0, ...heatmap.flat().filter((c) => c.inRange).map((c) => c.value)),
    [heatmap],
  );

  if (error) {
    return (
      <div className="claude-monitor">
        <p className="claude-monitor__error">集計に失敗しました: {error}</p>
      </div>
    );
  }
  if (!stats) {
    return (
      <div className="claude-monitor">
        <p className="claude-monitor__empty">集計しています…</p>
      </div>
    );
  }

  const maxModel = Math.max(0, ...stats.models.map((m) => m.total));
  const nowMs = Date.now();

  return (
    <div className="claude-monitor claude-stats">
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
          <span className="claude-stats__value">{stats.messages.toLocaleString()}</span>
          <span className="claude-stats__sub">サブエージェントの分を含む</span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">アクティブ日数</span>
          <span className="claude-stats__value">{stats.activeDays}</span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">合計トークン数</span>
          <span className="claude-stats__value">{formatTokens(stats.totalTokens)}</span>
          <span className="claude-stats__sub">うちキャッシュ入力 {formatTokens(stats.tokens.cachedInput)}</span>
        </div>
        <div className="claude-stats__tile">
          <span className="claude-stats__label">推論出力トークン</span>
          <span className="claude-stats__value">{formatTokens(stats.tokens.reasoningOutput)}</span>
          <span className="claude-stats__sub">出力 {formatTokens(stats.tokens.output)} の内数</span>
        </div>
      </div>

      <section className="claude-stats__section">
        <h3 className="claude-stats__heading">レート制限(期間と端末の絞り込みに関係なく、最新の値)</h3>
        {stats.limits.length === 0 && <p className="claude-monitor__empty">まだ記録がありません。</p>}
        {stats.limits.map((l) => (
          <LimitView key={l.limitId} limit={l} nowMs={nowMs} />
        ))}
      </section>

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
                <span className="claude-stats__bar-fill" style={{ width: `${maxModel > 0 ? (m.total / maxModel) * 100 : 0}%` }} />
              </span>
              <span className="claude-stats__model-value">{formatTokens(m.total)}</span>
            </li>
          ))}
        </ul>
      </section>

      {stats.contextWindows.length > 0 && (
        <section className="claude-stats__section">
          <h3 className="claude-stats__heading">コンテキスト窓の大きさ</h3>
          <ul className="claude-stats__models">
            {stats.contextWindows.map((w) => (
              <li key={w.model} className="codex-stats__row">
                <span className="claude-stats__model-name">{w.model}</span>
                <span />
                <span className="claude-stats__model-value">{formatTokens(w.tokens)}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

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

export default CodexStatsView;
