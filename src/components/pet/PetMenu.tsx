import { useEffect, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { api } from "../../api/tauri";
import type { PetModeDto, PetModeSetting } from "../../types";
import "./PetMenu.css";

// ペットを右クリックしたときのメニュー(ペットの行動の設計書の9・10節)。外を押すと本体(pet/mod.rs)が閉じる。
const SETTINGS: { key: PetModeSetting; label: string }[] = [
  { key: "auto", label: "自動" },
  { key: "normal", label: "普段に固定" },
  { key: "work", label: "作業中に固定" },
];

function PetMenu() {
  const [state, setState] = useState<PetModeDto | null>(null);

  useEffect(() => {
    document.body.style.background = "transparent";
    api
      .petModeGet()
      .then(setState)
      .catch(() => undefined);
    let stop: (() => void) | undefined;
    // 開くたびに、いまのモードを受け取る
    getCurrentWebviewWindow()
      .listen<PetModeDto>("pet://menu-open", (e) => setState(e.payload))
      .then((u) => {
        stop = u;
      });
    return () => stop?.();
  }, []);

  return (
    <div className="pet-menu" onContextMenu={(e) => e.preventDefault()}>
      <button type="button" className="pet-menu__item" onClick={() => api.petPickStart().catch(() => undefined)}>
        ここに移動して
      </button>
      <div className="pet-menu__label">モード(いまは{state?.mode === "work" ? "作業中" : "普段"})</div>
      {SETTINGS.map((s) => (
        <button
          key={s.key}
          type="button"
          className="pet-menu__item"
          aria-pressed={state?.setting === s.key}
          onClick={() => api.petModeSet(s.key).catch(() => undefined)}
        >
          <span className="pet-menu__mark">{state?.setting === s.key ? "●" : ""}</span>
          {s.label}
        </button>
      ))}
    </div>
  );
}

export default PetMenu;
