import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfigDto,
  AppConfigUpdate,
  ClaudeSession,
  ClaudeStats,
  ClaudeStatsRange,
  DiagnoseReport,
  AppMode,
  Handoff,
  LibraryFilter,
  EjectOutcome,
  EjectPreview,
  ExtensionInfo,
  Notice,
  PetBehavior,
  PetBehaviorDto,
  PetModeDto,
  PetModeSetting,
  PetSeenApp,
  PetStatus,
  SelfTestMode,
  SidebarLayout,
  KnowledgeResult,
  LibraryFile,
  ModelInfo,
  ModelSwitchEstimate,
  ModelSwitchResult,
  PassiveRecallStats,
  PendingAction,
  PendingItems,
  RagasHistory,
  SearchLibraryResult,
  ShelfDoc,
  SourceFrontmatter,
  SystemDoc,
  SystemInfo,
  TtsFailure,
} from "../types";

export interface ServiceStatus {
  name: string;
  port: number;
  started: boolean;
  healthy: boolean;
  error?: string;
  // 外部AIモード(詩織Ver4.0)で、意図して起動しなかった。失敗ではない。
  skipped: boolean;
}

export function isServiceFailed(s: ServiceStatus): boolean {
  return !s.healthy && !s.skipped;
}

// 仕様書v2.0「9. Tauriコマンド」で定義した各コマンドの型付きラッパー。
// Tauriのinvoke()を直接importするのはこのファイルだけに限定する。
export const api = {
  startBackendServices: () =>
    invoke<ServiceStatus[]>("start_backend_services"),


  // Function Calling方式(案②)により、モード指定は不要。単一プロンプト+ツール定義で
  // LLMが自律的に判断する。detectedModeは結果として「どのツールが呼ばれたか」を表す。
  sendMessage: (text: string) =>
    invoke<{
      reply: string;
      sources?: KnowledgeResult[];
      detectedMode: string;
    }>("send_message", { text }),

  startRecording: () => invoke<void>("start_recording"),

  stopRecordingAndTranscribe: () =>
    invoke<{ text: string }>("stop_recording_and_transcribe"),

  synthesizeSpeech: (text: string) =>
    invoke<void>("synthesize_speech", { text }),

  // コントロールパネルの「システム状態」向け。1〜2秒間隔でポーリングされる想定。
  getSystemInfo: () => invoke<SystemInfo>("get_system_info"),

  // コントロールパネルの「音声設定」試し読み用。config.jsonへの保存はしない。
  previewVoice: (text: string, lengthScale: number, noiseScale: number, noiseW: number) =>
    invoke<void>("preview_voice", {
      text,
      lengthScale,
      noiseScale,
      noiseW,
    }),

  getConfig: () => invoke<AppConfigDto>("get_config"),

  listClaudeSessions: () => invoke<ClaudeSession[]>("list_claude_sessions"),
  // Claudeの知らせから、作業が終わったセッションのアプリを前に出す。出せたかを返す(詩織Ver6.0)
  openClaudeSession: (sessionId: string) => invoke<boolean>("open_claude_session", { sessionId }),

  // 画面側の未処理のエラーをdata/logs/へ記録する(詩織Ver4.0)。
  logFrontendError: (message: string) => invoke<void>("log_frontend_error", { message }),
  // 画面の操作(タブの移動など)をログに残す(詩織Ver6.0の段階1)。部品名は本体側でUIに固定される。
  logUiEvent: (event: string, fields?: Record<string, string>) =>
    invoke<void>("log_ui_event", { event, fields: fields ?? null }),

  // 拡張機能(詩織Ver6.0の段階1)。一覧は起動時に読み込んだもの。承認すると、新しい一覧が返る。
  listExtensions: () => invoke<ExtensionInfo[]>("list_extensions"),
  approveExtension: (name: string) => invoke<ExtensionInfo[]>("approve_extension", { name }),
  // 画面の部品の拡張機能からの頼みごと。断られたときは{ code, message }でrejectする。
  extCall: (name: string, method: string, params: unknown) =>
    invoke<unknown>("ext_call", { name, method, params }),
  // 本体の画面から、拡張機能のprovidesにある操作を呼ぶ(段階1の仮の入り口で確かめるため)。
  callExtension: (name: string, method: string, params?: unknown) =>
    invoke<unknown>("call_extension", { name, method, params: params ?? null }),

  // 左の縦の列の並びと非表示(詩織Ver6.0の段階2)。data/ui/sidebar.jsonに置く。
  getSidebarLayout: () => invoke<SidebarLayout>("get_sidebar_layout"),
  setSidebarLayout: (layout: SidebarLayout) => invoke<void>("set_sidebar_layout", { layout }),
  // data/ui/icons/に差し替えのPNGがある、本体の画面のid
  listIconOverrides: () => invoke<string[]>("list_icon_overrides"),

  // 自己確認の起動(詩織Ver6.0の段階2)。ふつうの起動では、書く・閉じるは何もしない。
  selfTestMode: () => invoke<SelfTestMode>("self_test_mode"),
  selfTestLog: (item: string, result: "OK" | "NG", fields?: Record<string, string>) =>
    invoke<void>("self_test_log", { item, result, fields: fields ?? null }),
  selfTestFinish: (ok: number, ng: number) => invoke<void>("self_test_finish", { ok, ng }),
  // 自己確認(段階3): クリップボードの読み戻しと、Claude・要確認の試しの知らせ
  selfTestClipboardText: () => invoke<string | null>("self_test_clipboard_text"),
  // 診断(詩織Ver6.0の段階6)
  diagnoseReport: () => invoke<DiagnoseReport>("diagnose_report"),
  diagnoseCopy: (text: string) => invoke<void>("diagnose_copy", { text }),
  selfTestNotify: (kind: "claude" | "pending") => invoke<number | null>("self_test_notify", { kind }),
  // 自己確認(段階6): 起動時の自動修復が壊したものを直すか。[合否, 欄]
  selfTestRepair: () => invoke<[boolean, Record<string, string>] | null>("self_test_repair"),
  // ペットの形(詩織Ver6.0の段階4)
  petEnter: () => invoke<void>("pet_enter"),
  petLeave: () => invoke<void>("pet_leave"),
  petPress: () => invoke<void>("pet_press"),
  petBubbleDone: () => invoke<void>("pet_bubble_done"),
  petOpenSource: (notice: Notice) => invoke<void>("pet_open_source", { notice }),
  petPanelReport: (report: { items?: string[]; tool?: { name: string; ok: number; ng: number } }) =>
    invoke<void>("pet_panel_report", report),
  petClosePanel: () => invoke<void>("pet_close_panel"),
  petStatus: () => invoke<PetStatus>("pet_status"),
  petSelftestPanel: (tool?: string) => invoke<void>("pet_selftest_panel", { tool: tool ?? null }),
  petSelftestClosePanel: () => invoke<void>("pet_selftest_close_panel"),
  // 右クリックのメニューと、行き先を選ぶ画面(ペットの行動の設計書の9節)
  petOpenMenu: () => invoke<void>("pet_open_menu"),
  petPickStart: () => invoke<void>("pet_pick_start"),
  petPickChoose: () => invoke<void>("pet_pick_choose"),
  petPickCancel: () => invoke<void>("pet_pick_cancel"),
  // 普段と作業中のモード(ペットの行動の設計書の10節)
  petModeGet: () => invoke<PetModeDto>("pet_mode_get"),
  petModeSet: (setting: PetModeSetting) => invoke<void>("pet_mode_set", { setting }),
  // ペットの行動の設定(ペットの行動の設計書の6節)。保存するとペットの形のままでもすぐ効く
  petBehaviorGet: () => invoke<PetBehaviorDto>("pet_behavior_get"),
  petBehaviorSet: (behavior: PetBehavior) => invoke<PetBehavior>("pet_behavior_set", { behavior }),
  petSeenApps: () => invoke<PetSeenApp[]>("pet_seen_apps"),
  // forceがfalseなら、前回の取り込みから間もないときは省く。
  refreshClaudeStats: (force: boolean) => invoke<unknown>("refresh_claude_stats", { force }),
  // hostがnullなら全端末の合算。
  getClaudeStats: (range: ClaudeStatsRange, host: string | null) =>
    invoke<ClaudeStats>("get_claude_stats", { range, host }),

  ejectPreview: () => invoke<EjectPreview>("eject_preview"),
  ejectExecute: () => invoke<EjectOutcome>("eject_execute"),

  setConfig: (update: AppConfigUpdate) => invoke<void>("set_config", { update }),

  restartLlmServices: () => invoke<ServiceStatus[]>("restart_llm_services"),
  // 会話モード/外部AIモードを切り替える。返すのは切り替え後の会話用LLMの状態。
  setAppMode: (mode: AppMode) => invoke<ServiceStatus>("set_app_mode", { mode }),

  // 起動画面で「再試行」ボタンから呼ばれる、RAGサーバー単体の再起動。
  retryRagService: () => invoke<ServiceStatus>("retry_rag_service"),

  restartApp: () => invoke<void>("restart_app"),

  listAvailableModels: () => invoke<ModelInfo[]>("list_available_models"),

  estimateModelSwitch: (fileName: string) =>
    invoke<ModelSwitchEstimate>("estimate_model_switch", { fileName }),

  switchModel: (fileName: string) => invoke<ModelSwitchResult>("switch_model", { fileName }),

  // KnowledgePanelの参照情報クリック時、元のMarkdown全文を取得する(v1.0機能1)。
  getSourceDocument: (sourceCategory: string, source: string) =>
    invoke<string>("get_source_document", { sourceCategory, source }),

  // 素材(.metaの実体、詩織Ver3.5)を既定のアプリで開く/フォルダで表示する。
  // パスがlibrary/配下かの検証はRust側で行う。
  openLibraryAsset: (path: string) => invoke<void>("open_library_asset", { path }),
  revealLibraryAsset: (path: string) => invoke<void>("reveal_library_asset", { path }),

  // 規約画面(詩織Ver4.1)。詩織のシステムを決めている書類を読み取り専用で取得する。
  // 読めるのはRust側の許可リストにある書類だけ。
  listSystemDocs: () => invoke<SystemDoc[]>("list_system_docs"),
  readSystemDoc: (id: string) => invoke<string>("read_system_doc", { id }),
  // 知識の棚タブ。一覧と本文はファイルを直接読み、検索だけRAGを使う(関連のあるファイル名を返す)。
  listShelfDocs: () => invoke<ShelfDoc[]>("list_shelf_docs"),
  readShelfDoc: (name: string) => invoke<string>("read_shelf_doc", { name }),
  searchShelf: (query: string) => invoke<string[]>("search_shelf", { query }),

  // スタンドアロン図書館UI向け、検索を経由しない蔵書全件の一覧取得。
  // Phase 7(1冊=1ファイル表示単位への変更)により、ファイル単位に集約された
  // 結果を返す(2026-08-12、図書館ビジョン統合仕様書3-2)。
  listAllKnowledge: () => invoke<LibraryFile[]>("list_all_knowledge"),

  // ライブラリウィンドウの検索結果ベースUI向け(Ver3.0、UI改善4-2節)。
  // スコア閾値による足切りは行わないため、offsetを増やして同じクエリで
  // 呼び直すことでページングする(返却件数がlimit未満なら候補打ち止め)。
  searchLibrary: (query: string, limit: number, offset: number, filter: LibraryFilter) =>
    invoke<SearchLibraryResult[]>("search_library", { query, limit, offset, filter }),
  listLibraryProjects: () => invoke<string[]>("list_library_projects"),
  // 申し送りを持つjournalが無ければnull。
  getLatestHandoff: () => invoke<Handoff | null>("get_latest_handoff"),
  getPortableStorage: () => invoke<{ freeGb: number; totalGb: number }>("get_portable_storage"),

  // 記事詳細画面向け(Ver3.0、UI改善4-2節)。frontmatterの構造化フィールドを返す。
  getSourceFrontmatter: (sourceCategory: string, source: string) =>
    invoke<SourceFrontmatter>("get_source_frontmatter", { sourceCategory, source }),

  // 要確認UI向け(Phase 8)。pending/deferredなタグ・プロジェクト・inbox記録の
  // 一覧取得と、承認/却下/保留の反映。
  listPendingItems: () => invoke<PendingItems>("list_pending_items"),
  resolvePendingTag: (canonical: string, action: PendingAction) =>
    invoke<void>("resolve_pending_tag", { canonical, action }),
  resolvePendingProject: (id: string, action: PendingAction) =>
    invoke<void>("resolve_pending_project", { id, action }),
  resolvePendingInboxItem: (filename: string, action: PendingAction) =>
    invoke<void>("resolve_pending_inbox_item", { filename, action }),

  // 以下3つはコントロールパネルの「デバッグ」画面向け(v1.0機能2)。
  getTtsFailures: () => invoke<TtsFailure[]>("get_tts_failures"),

  getPassiveRecallStats: () => invoke<PassiveRecallStats>("get_passive_recall_stats"),

  getRagasHistory: () => invoke<RagasHistory>("get_ragas_history"),
};
