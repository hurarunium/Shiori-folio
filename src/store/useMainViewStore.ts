import { create } from "zustand";
import { api } from "../api/tauri";

// 本体の画面のid(詩織Ver6.0の段階2で、上のタブから左の縦の列に移した)。
export type BuiltinView =
  | "home"
  | "conversation"
  | "tasks"
  | "library"
  | "library-browse"
  | "shelf"
  | "rules"
  | "claude"
  | "extensions"
  | "settings";

// 選んでいる画面。画面の拡張機能は "ext:{名前}"。永続化はせず、起動のたびにホームから始める。
export type MainView = BuiltinView | `ext:${string}`;

// AIの画面(idは"claude"のまま)の中のサブタブ(詩織Ver7.0.2)。
export type AiTab = "claude" | "codex";

interface MainViewStore {
  active: MainView;
  setActive: (view: MainView) => void;
  aiTab: AiTab;
  setAiTab: (tab: AiTab) => void;
  // Claude向けのリンクから開くとき。前にCodexを見ていても、Claudeのサブタブで開く。
  openClaude: () => void;
  // 小道具の枠で開いている拡張機能の名前(開ける枠は1つ)
  openTool: string | null;
  // 同じ小道具をもう一度押すと閉じ、別の小道具を押すと入れ替わる
  toggleTool: (name: string) => void;
  closeTool: () => void;
  // 閉じずに開く(知らせの札から小道具へ移るとき)
  showTool: (name: string) => void;
  // ホームの検索欄から図書館へ渡す検索語。図書館が受け取ったらnullに戻す。
  libraryQuery: string | null;
  searchInLibrary: (query: string) => void;
  clearLibraryQuery: () => void;
  // ホームの札からタスクの画面へ渡すタスクのid。タスクの画面が受け取ったらnullに戻す。
  taskToOpen: string | null;
  openTask: (id: string) => void;
  clearTaskToOpen: () => void;
}

// 画面の移動をログに残す。記録に失敗しても、画面の切り替えは止めない。
function logMove(from: MainView, to: MainView) {
  if (from !== to) {
    api.logUiEvent("画面を移った", { from, to }).catch(() => undefined);
  }
}

export const useMainViewStore = create<MainViewStore>((set, get) => ({
  active: "home",
  setActive: (view) => {
    logMove(get().active, view);
    set({ active: view });
  },
  aiTab: "claude",
  setAiTab: (tab) => {
    if (get().aiTab !== tab) api.logUiEvent("AIのサブタブを移った", { to: tab }).catch(() => undefined);
    set({ aiTab: tab });
  },
  openClaude: () => {
    logMove(get().active, "claude");
    set({ active: "claude", aiTab: "claude" });
  },
  openTool: null,
  toggleTool: (name) => {
    const next = get().openTool === name ? null : name;
    api.logUiEvent(next ? "小道具を開いた" : "小道具を閉じた", { name }).catch(() => undefined);
    set({ openTool: next });
  },
  closeTool: () => {
    const current = get().openTool;
    if (current) api.logUiEvent("小道具を閉じた", { name: current }).catch(() => undefined);
    set({ openTool: null });
  },
  showTool: (name) => {
    if (get().openTool === name) return;
    api.logUiEvent("小道具を開いた", { name }).catch(() => undefined);
    set({ openTool: name });
  },
  libraryQuery: null,
  searchInLibrary: (query) => {
    logMove(get().active, "library");
    set({ active: "library", libraryQuery: query });
  },
  clearLibraryQuery: () => set({ libraryQuery: null }),
  taskToOpen: null,
  openTask: (id) => {
    logMove(get().active, "tasks");
    set({ active: "tasks", taskToOpen: id });
  },
  clearTaskToOpen: () => set({ taskToOpen: null }),
}));
