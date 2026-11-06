import type { ExtensionInfo, SidebarLayout } from "../types";
import type { BuiltinView, MainView } from "../store/useMainViewStore";
import { extensionUrl, EXT_ORIGIN } from "../components/extensions/ExtensionFrame";

// 左の縦の列の項目(詩織Ver6.0の段階2)。本体の画面と拡張機能を、同じ形の1つの一覧にする。
// 並べ替え・非表示・自己確認は、この一覧に対して行う。
//   - screen: 線の上の「画面」。選ぶと画面全体が切り替わる
//   - tool:   線の下の「小道具」。いまの画面の横の小さな枠で開く
//   - bottom: いちばん下(拡張機能・設定)。並べ替えも非表示もしない(隠すと戻れなくなるため)
export type SidebarGroup = "screen" | "tool" | "bottom";

export interface SidebarItem {
  id: MainView;
  label: string;
  group: SidebarGroup;
  // アイコンのURL。無ければ名前の頭の1文字を出す
  icon: string | null;
  // 拡張機能の項目なら、その名前と入口
  ext: { name: string; entry: string; selftest: boolean } | null;
}

// 同梱の仮のドット絵(src/assets/icons/{id}.png)。data/ui/icons/に同じ名前のPNGを置くと、そちらを使う。
const BUNDLED_ICONS = import.meta.glob("../assets/icons/*.png", { eager: true, import: "default" }) as Record<
  string,
  string
>;

function bundledIcon(id: string): string | null {
  return BUNDLED_ICONS[`../assets/icons/${id}.png`] ?? null;
}

const BUILTIN: { id: BuiltinView; label: string; group: SidebarGroup }[] = [
  { id: "home", label: "ホーム", group: "screen" },
  { id: "conversation", label: "会話", group: "screen" },
  { id: "tasks", label: "タスク", group: "screen" },
  { id: "library", label: "図書館", group: "screen" },
  { id: "library-browse", label: "全件閲覧", group: "screen" },
  { id: "shelf", label: "知識の棚", group: "screen" },
  { id: "rules", label: "規約・記憶", group: "screen" },
  { id: "claude", label: "Claude", group: "screen" },
  { id: "extensions", label: "拡張機能", group: "bottom" },
  { id: "settings", label: "設定", group: "bottom" },
];

export function builtinLabel(id: MainView): string {
  return BUILTIN.find((b) => b.id === id)?.label ?? id;
}

// 列に出す拡張機能: 画面の部品で、使える状態で、ウィンドウで使えるもの(ペットは段階4)。
function extensionItems(list: ExtensionInfo[]): SidebarItem[] {
  return list
    .filter((e) => e.status === "ready" && e.kind === "ui" && e.name && e.entry && e.places.includes("window"))
    .map((e) => ({
      id: `ext:${e.name}` as MainView,
      label: e.title ?? (e.name as string),
      group: e.slot,
      icon: e.icon ? extensionUrl(e.name as string, e.icon) : null,
      ext: { name: e.name as string, entry: e.entry as string, selftest: e.selftest },
    }));
}

// 全部の項目を、既定の並び(本体の画面→拡張機能)で返す。
export function allItems(extensions: ExtensionInfo[], iconOverrides: string[]): SidebarItem[] {
  const builtin: SidebarItem[] = BUILTIN.map((b) => ({
    ...b,
    icon: iconOverrides.includes(b.id) ? `${EXT_ORIGIN}/_shiori/icons/${b.id}.png` : bundledIcon(b.id),
    ext: null,
  }));
  return [...builtin, ...extensionItems(extensions)];
}

// 記録の並びに合わせて並べる。記録に無い項目(新しく足した拡張機能)は、その仲間のいちばん下に足す。
// 記録にあって一覧に無いもの(消えた拡張機能)は無視する。
export function arrange(items: SidebarItem[], layout: SidebarLayout): SidebarItem[] {
  const rank = (item: SidebarItem, index: number) => {
    const i = layout.order.indexOf(item.id);
    return i >= 0 ? i : layout.order.length + index;
  };
  const groups: SidebarGroup[] = ["screen", "tool", "bottom"];
  return groups.flatMap((g) => {
    const members = items.map((item, index) => ({ item, index })).filter(({ item }) => item.group === g);
    if (g === "bottom") return members.map(({ item }) => item);
    return members.sort((a, b) => rank(a.item, a.index) - rank(b.item, b.index)).map(({ item }) => item);
  });
}

export function isHideable(item: SidebarItem): boolean {
  return item.group !== "bottom";
}

export function visibleItems(items: SidebarItem[], layout: SidebarLayout): SidebarItem[] {
  return items.filter((item) => !isHideable(item) || !layout.hidden.includes(item.id));
}
