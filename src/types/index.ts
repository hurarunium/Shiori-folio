// 単一人格+Function Calling+常時バックグラウンド検索(passive recall)方式では、
// モードをユーザーが選ぶ概念は存在しない。ActiveToolは「今まさに実行中の
// ツール」を表す一時的な状態(ActivityIndicator/OrbCoreの表示用)。
// search_knowledge/memo実行中はそのツール名になり、それ以外は"idle"。
// タスク管理機能(update_task)は廃止済み(メモ機能に置き換え、2026-08-08)。
export type ActiveTool = "idle" | "search_knowledge" | "memo";

export type OrbStatus = "idle" | "listening" | "thinking" | "speaking";

export interface ShioriMessage {
  role: "user" | "assistant";
  text: string;
}

export interface KnowledgeResult {
  id: string;
  text: string;
  source: string;
  heading: string;
  sourceCategory: string;
}

// 規約画面(詩織Ver4.1)の書類1件。idから本文を取得する(許可リストの書類のみ)。
export interface SystemDoc {
  id: string;
  // 見出し。"memory" | "skill" | "rules" | "behavior"。
  tab: string;
  // 見出しの中の小見出し。
  sub: string;
  // 技術別の参照ファイルが持つ、親スキルのid。
  parentId: string | null;
  // memoryだけが持つ、frontmatterの更新日時。
  updated: string | null;
  label: string;
  fileName: string;
  // 表示方法。"markdown" | "yaml" | "json" | "text"。
  format: string;
  exists: boolean;
  size: number;
  mtime: number;
}

// 知識の棚タブ(Ver5.0候補)の記事1件。frontmatterの項目だけで、本文は別に取得する。
export interface ShelfDoc {
  name: string;
  title: string;
  topic: string;
  learned: string;
  summary: string;
  source: string;
  mtime: number;
}

// スタンドアロン図書館UI(Phase 7、1冊=1ファイルの表示単位)向け。チャンクの
// 本文は含まず、ファイル内の見出し一覧だけを持つ。
export interface LibraryFile {
  source: string;
  sourceCategory: string;
  headings: string[];
  // frontmatterのtitle(無ければ空文字列)・実ファイルへの絶対パス
  // (2026-09-16追加、エクスプローラー風UI刷新向け)。
  title: string;
  path: string;
  // .meta(素材のサイドカー)のとき対応する実体の絶対パス。.metaでない・実体が
  // 無い(孤児)場合は空文字列(詩織Ver3.5)。
  assetPath: string;
  // library_rootからの相対パス(/区切り)。フォルダツリー構築に使う。
  relativePath: string;
  // ファイルの更新日時(Unixタイムスタンプ、秒)。
  mtime: number;
}

// ライブラリウィンドウの検索結果ベースUI(Ver3.0、UI改善4-2節)向け。
// search_libraryはファイル単位に集約された、ヒットした見出しとスコアの
// リストを返す(本文は含まない)。
export interface SearchLibraryHeading {
  heading: string;
  rerankScore: number;
}

export interface SearchLibraryResult {
  source: string;
  sourceCategory: string;
  headings: SearchLibraryHeading[];
  bestScore: number;
  // 実ファイルへの絶対パス・frontmatterのtitle(2026-09-16追加)。
  path: string;
  // .metaのとき対応する実体の絶対パス(無ければ空文字列、詩織Ver3.5)。
  assetPath: string;
  title: string;
  // 一覧に添える属性と冒頭の文(詩織Ver3.9)。記事を読めなかったときは空文字列。
  summary: string;
  project: string;
  entryKind: string;
  status: string;
  date: string;
}

// 図書館の検索の絞り込み(詩織Ver3.9)。空の項目は絞り込まない。
export interface LibraryFilter {
  // falseなら、アーカイブ(90-archive)とstatusがoutdated/deprecatedの記録を除く。
  includeStale: boolean;
  sourceCategories: string[];
  project: string | null;
  kind: string | null;
}

// ホームの申し送り(詩織Ver3.9。Ver4.1でmemoryを優先)。fromMemoryのときはmemoryの
// 「やること」(todos)と「申し送り・待ち」(notes)、そうでなければjournalの申し送り(items)。
// dateはmemoryなら最終更新日(updated)。
export interface Handoff {
  title: string;
  project: string;
  date: string;
  source: string;
  sourceCategory: string;
  items: string[];
  fromMemory: boolean;
  todos: string[];
  notes: string[];
}

// 記事詳細画面(Ver3.0、UI改善4-2節)向け。frontmatterの構造化フィールド。
export interface SourceFrontmatter {
  title: string | null;
  type: string | null;
  tags: string[];
  project: string | null;
  summary: string | null;
  index: boolean;
  status: string;
  related: string[];
}

// 要確認UI(Phase 8)向け。tags.yaml/projects.yamlのstatus、inboxファイルの
// review_statusはいずれも"pending"(未着手)/"deferred"(保留中)のいずれか
// (confirmed済みのものはバックエンド側で一覧から除外されている)。
export type PendingStatus = "pending" | "deferred";

export interface PendingTag {
  canonical: string;
  status: PendingStatus;
}

export interface PendingProject {
  id: string;
  status: PendingStatus;
}

export interface PendingInboxItem {
  filename: string;
  title: string | null;
  reason: string | null;
  reviewStatus: PendingStatus | null;
}

export interface PendingItems {
  tags: PendingTag[];
  projects: PendingProject[];
  inbox: PendingInboxItem[];
}

// resolvePendingTag/resolvePendingProject/resolvePendingInboxItem共通のaction。
// confirm=承認、reject=却下(エントリ/ファイル削除)、defer=保留。
export type PendingAction = "confirm" | "reject" | "defer";

export type TagVariant = "warm" | "teal" | "cyan";

export interface GpuInfo {
  name: string;
  vramUsedMb: number;
  vramTotalMb: number;
  temperatureC: number | null;
}

export interface ServiceInfo {
  name: string;
  running: boolean;
  vramMb: number | null;
  ramMb: number | null;
  modelName: string | null;
}

export interface DiskThroughput {
  readMbPerSec: number;
  writeMbPerSec: number;
}

export interface StorageInfo {
  drive: string;
  kind: string;
  fileSystem: string;
  totalGb: number;
  freeGb: number;
  isRemovable: boolean;
}

export interface PowerInfo {
  onBattery: boolean;
  batteryPercent: number | null;
}

export interface CpuDetail {
  model: string;
  physicalCores: number;
  logicalCores: number;
  frequencyMhz: number;
}

export interface SystemInfo {
  gpu: GpuInfo | null;
  ramUsedMb: number;
  ramTotalMb: number;
  cpuUsagePercent: number;
  os: string;
  // "windows" / "macos" / "linux"。VRAM表示可否等のOS判定に使う。
  platform: string;
  cpu: CpuDetail;
  services: ServiceInfo[];
  diskThroughput: DiskThroughput | null;
  storage: StorageInfo | null;
  power: PowerInfo | null;
}

export interface AppConfigDto {
  hotkey: string;
  llmPort: number;
  llmContextSize: number | null;
  embeddingPort: number;
  sttPort: number;
  ragPort: number;
  passiveRecallThreshold: number;
  lengthScale: number;
  noiseScale: number;
  noiseW: number;
  showYear: boolean;
  showMonth: boolean;
  showDay: boolean;
  showWeekday: boolean;
  showSeconds: boolean;
  mode: AppMode;
  // ログに「詳細」の重さも書くか(詩織Ver6.0の段階1)
  logDetail: boolean;
  // 水紋と星の動きを抑えるか(詩織Ver6.0の段階2)。nullならOSの「視差効果を減らす」に従う
  reduceMotion: boolean | null;
  // 知らせの種類ごとに音を鳴らすか(詩織Ver6.0の段階3)
  notifySound: NotifySound;
  // ペットの形の切り替えのホットキーと、眠るまでの分(詩織Ver6.0の段階4)
  petHotkey: string;
  petSleepMinutes: number;
  // 本と小道具の枠の大きさ(%)
  petPanelPercent: number;
}

// ペットの形(詩織Ver6.0の段階4)。本体(pet/mod.rs)が決めた、いまの絵のコマ
export type PetState =
  | "appear"
  | "normal"
  | "sleep"
  | "wake"
  | "notify"
  | "happy"
  | "react"
  | "dangle"
  | "fly"
  | "land"
  // 足場から足場へ跳ぶ(ペットの行動の設計書)
  | "jump";

export interface PetFrame {
  state: PetState;
  // sit: ウィンドウの縁に座る(ペットの行動の設計書)
  activity: "walk" | "idle" | "read" | "sit";
  anim: string;
  file: string;
  frames: number;
  frame: number;
  facing: number;
  scale: number;
}

export interface PetStatus {
  mode: "window" | "pet";
  state: PetState | null;
  anim: string | null;
  scale: number;
  position: [number, number] | null;
  size: [number, number] | null;
  floor: number | null;
  onFloor: boolean;
  mainVisible: boolean;
  petVisible: boolean;
  panelVisible: boolean;
  bubbleVisible: boolean;
  panelItems: string[];
  panelTool: { name: string; ok: number; ng: number } | null;
  idleSeconds: number | null;
  spritesOverridden: boolean;
  hitOutside: boolean;
  hitCenter: boolean;
  // いまの居場所(床かウィンドウか)
  stand: { owner: { kind: "floor"; screen: number } | { kind: "window"; id: number }; dx: number } | null;
  // ペットの行動の設計書のPhase 1: 全部の画面・ほかのアプリのウィンドウ(前面から順)・足場
  screens: { full: PetRect; work: PetRect }[];
  windows: { id: number; app: string; rect: PetRect; focused: boolean }[];
  platforms: { owner: { kind: "floor"; screen: number } | { kind: "window"; id: number }; y: number; x0: number; x1: number }[];
  windowsMs: number;
}

export interface PetRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

// 知らせの種類(詩織Ver6.0の段階3)。タイマーの終わり・Claudeの作業の終わり・要確認・拡張機能からの知らせ
export type NoticeKind = "timer" | "claude" | "pending" | "extension";

export type NotifySound = Record<NoticeKind, boolean>;

// 本体の知らせの入口(notify.rs)から届く知らせ1つ
export interface Notice {
  id: number;
  kind: NoticeKind;
  title: string;
  body: string;
  source: string | null;
  at: number;
}

// 会話モード/外部AIモード(詩織Ver4.0)。
export type AppMode = "conversation" | "external";

// get_config()の全項目をそのままOptionalにした部分更新用の型。
export type AppConfigUpdate = Partial<AppConfigDto>;

export interface ModelInfo {
  fileName: string;
  sizeMb: number;
  vramEstimateGb: number;
  isMeasured: boolean;
  isCurrent: boolean;
}

export interface ModelSwitchEstimate {
  projectedVramPercent: number;
  isRisky: boolean;
}

export interface ModelSwitchResult {
  success: boolean;
  measuredVramGb: number | null;
  rolledBack: boolean;
  error: string | null;
}

export interface TtsFailure {
  error: string;
  createdAt: string;
}

export interface PassiveRecallStats {
  hits: number;
  total: number;
}

export interface RagasHistory {
  headers: string[];
  rows: string[][];
}

// Claudeモニター(Ver3.7)。state: working=作業中 / waiting=入力待ち / idle=待機 /
// ended=終了 / stale=更新が止まったまま(応答なし)。時刻はエポックミリ秒。
export type ClaudeSessionState = "working" | "waiting" | "idle" | "ended" | "stale";

export interface ClaudeSession {
  sessionId: string;
  project: string;
  title: string;
  host: string;
  state: ClaudeSessionState;
  startedAt: number;
  updatedAt: number;
}

// Claudeの利用統計(詩織Ver3.9)。「実際」は応答・発言のIDで重複を除いた値、
// 「アプリ式」はデスクトップアプリの統計パネルと同じく履歴の行ごとに数えた値。
export type ClaudeStatsRange = "all" | "days30" | "days7";
export type ClaudeStatsCounting = "actual" | "app";

export interface ClaudeTokens {
  input: number;
  cacheCreation: number;
  cacheRead: number;
  output: number;
}

export interface ClaudeStats {
  hosts: { host: string; updatedAt: number }[];
  since: string;
  until: string;
  sessions: number;
  messages: number;
  messagesApp: number;
  activeDays: number;
  actual: ClaudeTokens;
  app: ClaudeTokens;
  daily: { date: string; actual: number; app: number }[];
  models: { model: string; actual: number; app: number }[];
  warnings: string[];
}

// 取り外し機能。止める予定のもの(toStop)。
export interface EjectEntry {
  pid: number;
  parentPid: number | null;
  name: string;
  exe: string;
  // 止めたときの影響の説明(mcp_serverを使っているClaudeのセッション等)。
  note: string;
}

export interface EjectPlan {
  toStop: EjectEntry[];
}

export interface EjectPreview {
  plan: EjectPlan;
  // 止める予定のうち、詩織を閉じても残るもの(MCPなどが起動した共有デーモン)。
  leftoverAfterExit: EjectEntry[];
  // lsofで見つかった、ほかにSSDを開いているプロセス [pid, 名前]。
  otherHolders: [number, string][];
}

export interface EjectOutcome {
  stopped: number[];
  forced: number[];
  failed: number[];
}

// 拡張機能(詩織Ver6.0の段階1)。hub/mod.rsのExtensionDtoと同じ形。
export interface ExtensionUse {
  name: string;
  label: string;
}

export type ExtensionStatus = "ready" | "needs-approval" | "invalid";

export interface ExtensionInfo {
  folder: string;
  name: string | null;
  title: string | null;
  version: string | null;
  description: string | null;
  kind: "ui" | "program" | null;
  // 画面の部品の入口(拡張機能のフォルダからの相対パス)
  entry: string | null;
  places: ("window" | "pet")[];
  // 左の縦の列のどこに出すか(詩織Ver6.0の段階2)
  slot: "screen" | "tool";
  // アイコン(拡張機能のフォルダからの相対パス)
  icon: string | null;
  uses: ExtensionUse[];
  // ほかの拡張機能から呼ばれてよい操作
  provides: string[];
  // 自己確認の起動の合図に応える、自分の確かめを持つか(段階6)
  selftest: boolean;
  status: ExtensionStatus;
  // 初めての承認か(falseなら、使うものが増えたための承認)
  firstTime: boolean;
  addedUses: ExtensionUse[];
  errorCode: string | null;
  errorMessage: string | null;
  // 別のプログラムの拡張機能の、いまの動き(画面の部品はnull)
  runtime: ExtensionRunState | null;
  restarts: number;
}

// 左の縦の列の並びと非表示の記録(詩織Ver6.0の段階2、sidebar_layout.rsと同じ形)
export interface SidebarLayout {
  order: string[];
  hidden: string[];
}

// 診断の文章(詩織Ver6.0の段階6、lib.rsのDiagnoseDtoと同じ形)。ngは [NG] の行の数
export interface DiagnoseReport {
  text: string;
  ng: number;
}

// 自己確認の起動(詩織Ver6.0の段階2、self_test.rsと同じ形)
export interface SelfTestMode {
  enabled: boolean;
  // わざと壊す画面のid
  breakTarget: string | null;
}

export type ExtensionRunState =
  | { state: "starting" }
  | { state: "running" }
  | { state: "stopped"; reason: string };
