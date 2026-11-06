import { useEffect, useState, type ReactNode } from "react";
import { useMainViewStore, type MainView } from "../../store/useMainViewStore";
import { usePendingStore } from "../../store/usePendingStore";
import { useSidebarItems } from "../../store/useSidebarStore";
import { getSelfTestMode } from "../../lib/selfTest";
import ClaudeMonitorPanel from "../panels/ClaudeMonitorPanel";
import LibraryScreen from "../library/LibraryScreen";
import LibraryBrowseScreen from "../library/LibraryBrowseScreen";
import ControlPanel from "../controlpanel/ControlPanel";
import RulesScreen from "../rules/RulesScreen";
import ShelfScreen from "../shelf/ShelfScreen";
import TasksScreen from "../tasks/TasksScreen";
import ExtensionsScreen from "../extensions/ExtensionsScreen";
import ExtensionFrame from "../extensions/ExtensionFrame";
import ConversationScreen from "../conversation/ConversationScreen";
import ToolFrame from "./ToolFrame";
import HomeScreen from "./HomeScreen";
import ErrorBoundary from "../shared/ErrorBoundary";
import "./MainArea.css";

// 本体の画面の中身。検索バー固定+一覧スクロールのように、中身が自前でスクロールを管理する画面は
// selfScroll。それ以外は余白を付けて画面ごとスクロールさせる。
const BUILTIN_CONTENT: Partial<Record<MainView, { content: () => ReactNode; selfScroll: boolean }>> = {
  conversation: { content: () => <ConversationScreen />, selfScroll: true },
  tasks: { content: () => <TasksScreen />, selfScroll: true },
  library: { content: () => <LibraryScreen />, selfScroll: true },
  "library-browse": { content: () => <LibraryBrowseScreen />, selfScroll: true },
  shelf: { content: () => <ShelfScreen />, selfScroll: true },
  rules: { content: () => <RulesScreen />, selfScroll: true },
  claude: { content: () => <ClaudeMonitorPanel />, selfScroll: false },
  extensions: { content: () => <ExtensionsScreen />, selfScroll: false },
  settings: { content: () => <ControlPanel />, selfScroll: true },
};

// 自己確認の起動で、わざと壊した画面(NGになることを確かめる)。
function Broken(): ReactNode {
  throw new Error("自己確認のために、わざと壊した画面");
}

// 選んだ画面を1つ出す場所(詩織Ver6.0の段階2で、上のタブから左の縦の列に切り替えを移した)。
// 切り替えで検索結果や選んだページ、会話が消えないよう、全部の画面を常に出したまま、選ばれていない
// 画面はdisplay:noneで隠す。小道具は、右の端の枠(ToolFrame)で開く。
function MainArea() {
  const active = useMainViewStore((s) => s.active);
  const pending = usePendingStore((s) => s.items);
  const setPendingOpen = usePendingStore((s) => s.setOpen);
  const { all } = useSidebarItems();
  const [breakTarget, setBreakTarget] = useState<string | null>(null);

  useEffect(() => {
    getSelfTestMode().then((m) => setBreakTarget(m.breakTarget));
  }, []);

  const screens = all.filter((i) => i.group !== "tool");

  return (
    <div className="main-area">
      <div className="main-area__screens">
        {screens.map((item) => {
          const builtin = BUILTIN_CONTENT[item.id];
          const content =
            item.id === "home" ? (
              <HomeScreen pending={pending} onOpenPending={() => setPendingOpen(true)} />
            ) : builtin ? (
              builtin.content()
            ) : item.ext ? (
              <ExtensionFrame name={item.ext.name} entry={item.ext.entry} title={item.label} />
            ) : null;
          const selfScroll = item.id === "home" || !!item.ext || (builtin?.selfScroll ?? false);
          return (
            <div
              key={item.id}
              role="region"
              aria-label={item.label}
              data-view-id={item.id}
              className={`main-area__pane${selfScroll ? " main-area__pane--self-scroll" : ""}`}
              hidden={active !== item.id}
            >
              <ErrorBoundary label={item.label}>{breakTarget === item.id ? <Broken /> : content}</ErrorBoundary>
            </div>
          );
        })}
      </div>
      <ToolFrame />
    </div>
  );
}

export default MainArea;
