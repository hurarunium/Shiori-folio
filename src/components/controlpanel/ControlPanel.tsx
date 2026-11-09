import { useEffect, useState } from "react";
import { api } from "../../api/tauri";
import type { SystemInfo } from "../../types";
import "./ControlPanel.css";
import { AdvancedSettingsPage } from "./AdvancedSettingsPage";
import { ModelSwitchPage } from "./ModelSwitchPage";
import { SystemStatusPage } from "./SystemStatusPage";
import { VoiceSettingsPage } from "./VoiceSettingsPage";
import { EjectPage } from "../eject/EjectPage";
import { SidebarSettingsPage } from "./SidebarSettingsPage";
import { NotifySettingsPage } from "./NotifySettingsPage";
import { PetSettingsPage } from "./PetSettingsPage";
import { DebugPage } from "./DebugPage";
import { DiagnosePage } from "./DiagnosePage";
import { HISTORY_LENGTH } from "./shared";
import { useMainViewStore } from "../../store/useMainViewStore";

type NavKey = "system" | "sidebar" | "notify" | "pet" | "voice" | "advanced" | "model" | "eject" | "diagnose" | "debug";

const NAV_ITEMS: { key: NavKey; label: string; level: "safe" | "caution" | "danger" }[] = [
  { key: "system", label: "システム状態", level: "safe" },
  { key: "sidebar", label: "列と演出", level: "safe" },
  // 知らせの音(詩織Ver6.0の段階3)
  { key: "notify", label: "知らせ", level: "safe" },
  // ペットの形(詩織Ver6.0の段階4)
  { key: "pet", label: "ペット", level: "safe" },
  { key: "voice", label: "音声設定", level: "safe" },
  { key: "advanced", label: "詳細設定", level: "caution" },
  { key: "model", label: "モデル切替", level: "danger" },
  { key: "eject", label: "SSDの取り外し", level: "caution" },
  // 診断(詩織Ver6.0の段階6)。外部のAIに渡す文章を出す
  { key: "diagnose", label: "診断", level: "safe" },
  // デバッグ(詩織Ver6.0の段階2で、上のタブの右の全画面表示から、ほかのページと同じタブにした)
  { key: "debug", label: "デバッグ", level: "caution" },
];

// 設定画面(2026-08-12にウィンドウ化、詩織Ver3.9で右ゾーンのタブの1つにした)。
// 左ナビ+右コンテンツの2ペイン構造だけを持つ。
//
// 【重要】右ゾーン(MainArea)の画面は常時マウント方式のため(StartupScreenは
// 単なるCSSオーバーレイであり、その裏でこのコンポーネントもアプリ起動直後から
// マウントされている)、バックエンド起動(RAG/LLM等)を待たずにマウント時の
// useEffectが走る。ここでのgetSystemInfoはRust側ネイティブAPIのみに依存し
// サイドカー起動を待つ必要が無いため今は問題ないが、RAG/LLM依存の新規fetchを
// このコンポーネントや配下のページに追加する場合は、LibraryScreen.tsxと同様に
// useShioriStore.getState().ensureBackendServicesStarted()の完了を待ってから
// 呼ぶこと(でないと起動直後は必ず失敗する)。
function ControlPanel() {
  const [activeNav, setActiveNav] = useState<NavKey>("system");
  const [info, setInfo] = useState<SystemInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  // モデル切替の実行中は、途中で画面を離れられると状態把握が難しくなるため
  // サイドバーのナビゲーション・会話画面への「戻る」を一時的に無効化する。
  const [isSwitchingModel, setIsSwitchingModel] = useState(false);

  // 瞬間的なピーク(録音中だけVRAMが跳ねる等)を見逃さないための直近推移バッファ。
  // グラフ描画ライブラリは使わず、配列をそのままSVGのpolylineに渡す。
  const [vramHistory, setVramHistory] = useState<number[]>([]);
  const [ramHistory, setRamHistory] = useState<number[]>([]);
  const [tempHistory, setTempHistory] = useState<number[]>([]);

  // 画面はすべて出したまま隠す作り(MainArea)なので、隠れている間も取り続けないよう、
  // 設定画面が選ばれていて、窓が見えているときだけ取る(全プロセスの一覧の取り直しや、
  // Windowsの nvidia-smi の起動を、常駐中に1.5秒ごとに行わないため)。
  const isSettingsActive = useMainViewStore((s) => s.active === "settings");
  const [pageVisible, setPageVisible] = useState(() => !document.hidden);
  useEffect(() => {
    const onChange = () => setPageVisible(!document.hidden);
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);
  const shouldPoll = isSettingsActive && pageVisible;

  useEffect(() => {
    if (!shouldPoll) return;
    let cancelled = false;

    const poll = async () => {
      try {
        const result = await api.getSystemInfo();
        if (!cancelled) {
          setInfo(result);
          setError(null);
          if (result.gpu) {
            setVramHistory((h) => [...h, (result.gpu!.vramUsedMb / result.gpu!.vramTotalMb) * 100].slice(-HISTORY_LENGTH));
            if (result.gpu.temperatureC !== null) {
              setTempHistory((h) => [...h, result.gpu!.temperatureC!].slice(-HISTORY_LENGTH));
            }
          }
          setRamHistory((h) => [...h, (result.ramUsedMb / result.ramTotalMb) * 100].slice(-HISTORY_LENGTH));
        }
      } catch (err) {
        if (!cancelled) setError(String(err));
      }
    };

    poll();
    const timer = setInterval(poll, 1500);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [shouldPoll]);

  const vramPercent = info?.gpu
    ? Math.round((info.gpu.vramUsedMb / info.gpu.vramTotalMb) * 100)
    : null;
  const ramPercent = info ? Math.round((info.ramUsedMb / info.ramTotalMb) * 100) : null;

  return (
    <div className="control-panel">
      <aside className="control-panel__sidebar">
        {NAV_ITEMS.map((item) => (
          <div
            key={item.key}
            className={`control-panel__nav-item${activeNav === item.key ? " control-panel__nav-item--active" : ""}${isSwitchingModel ? " control-panel__nav-item--disabled" : ""}`}
            data-nav-key={item.key}
            onClick={() => !isSwitchingModel && setActiveNav(item.key)}
            title={item.label}
          >
            <span className={`control-panel__dot control-panel__dot--${item.level}`} />
            <span className="control-panel__nav-label">{item.label}</span>
          </div>
        ))}
      </aside>

      <main className="control-panel__main">
        {activeNav === "system" && (
          <SystemStatusPage
            info={info}
            error={error}
            vramPercent={vramPercent}
            ramPercent={ramPercent}
            vramHistory={vramHistory}
            ramHistory={ramHistory}
            tempHistory={tempHistory}
          />
        )}
        {activeNav === "sidebar" && <SidebarSettingsPage />}
        {activeNav === "notify" && <NotifySettingsPage />}
        {activeNav === "pet" && <PetSettingsPage />}
        {activeNav === "voice" && <VoiceSettingsPage />}
        {activeNav === "advanced" && <AdvancedSettingsPage />}
        {activeNav === "eject" && <EjectPage />}
        {activeNav === "diagnose" && <DiagnosePage />}
        {activeNav === "debug" && <DebugPage />}
        {activeNav === "model" && (
          <ModelSwitchPage onSwitchingChange={setIsSwitchingModel} platform={info?.platform ?? null} />
        )}
      </main>
    </div>
  );
}

export default ControlPanel;
