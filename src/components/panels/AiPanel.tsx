import { useMainViewStore, type AiTab } from "../../store/useMainViewStore";
import ClaudeMonitorPanel from "./ClaudeMonitorPanel";
import CodexStatsView from "./CodexStatsView";
import "./AiPanel.css";

const TABS: { id: AiTab; label: string }[] = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
];

// AIの画面(詩織Ver7.0.2)。Claude(稼働状況・利用統計)とCodex(利用統計)をサブタブで切り替える。
function AiPanel() {
  const visible = useMainViewStore((s) => s.active === "claude");
  const tab = useMainViewStore((s) => s.aiTab);
  const setTab = useMainViewStore((s) => s.setAiTab);

  return (
    <div className="ai-panel">
      <div className="ai-panel__tabs" role="tablist" aria-label="AIの種類">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={`ai-panel__tab${tab === t.id ? " ai-panel__tab--active" : ""}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>
      {tab === "claude" ? <ClaudeMonitorPanel /> : <CodexStatsView active={visible} />}
    </div>
  );
}

export default AiPanel;
