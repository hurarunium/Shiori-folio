// 利用統計(Claude・Codex)の画面で共有する、表示用の関数。

// 「すべて」でもヒートマップは直近1年分までにする(ウィンドウの幅に収めるため)。
const MAX_HEATMAP_WEEKS = 53;

export function formatTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(n);
}

function parseLocalDate(ymd: string): Date {
  const [y, m, d] = ymd.split("-").map(Number);
  return new Date(y, m - 1, d);
}

function toYmd(date: Date): string {
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${m}-${d}`;
}

export function formatDateTime(ms: number): string {
  const t = new Date(ms);
  return `${t.getMonth() + 1}/${t.getDate()} ${String(t.getHours()).padStart(2, "0")}:${String(t.getMinutes()).padStart(2, "0")}`;
}

export interface HeatCell {
  date: string;
  value: number;
  inRange: boolean;
}

// 期間の初日を含む週の日曜日から、今日を含む週の土曜日までを、週ごとの列に並べる。
// valuesは日付(YYYY-MM-DD)ごとの値。
export function buildHeatmap(since: string, untilDate: string, values: Map<string, number>): HeatCell[][] {
  const until = parseLocalDate(untilDate);
  const start = parseLocalDate(since);
  start.setDate(start.getDate() - start.getDay());
  const earliest = new Date(until);
  earliest.setDate(earliest.getDate() - earliest.getDay() - (MAX_HEATMAP_WEEKS - 1) * 7);
  if (start < earliest) start.setTime(earliest.getTime());

  const weeks: HeatCell[][] = [];
  for (const day = new Date(start); day <= until || day.getDay() !== 0; day.setDate(day.getDate() + 1)) {
    if (day.getDay() === 0) weeks.push([]);
    const date = toYmd(day);
    weeks[weeks.length - 1].push({
      date,
      value: values.get(date) ?? 0,
      inRange: date >= since && date <= untilDate,
    });
  }
  return weeks;
}

// 最大値に対する割合で4段階に分ける。0は記録なし。
export function heatLevel(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0;
  return Math.min(4, Math.ceil((value / max) * 4));
}
