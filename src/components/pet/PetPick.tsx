import { useEffect, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { api } from "../../api/tauri";
import "./PetPick.css";

// 行き先を選ぶ画面(ペットの行動の設計書の9節)。画面ごとに全体を覆う窓で、マウスの下の行き先は
// 本体(pet/mod.rs)が決めて送るので、ここは線を出すだけ。押すと決まり、Escか右クリックでやめる。
interface PickLine {
  x: number;
  y: number;
  w: number;
  label: string;
}

function PetPick() {
  const [line, setLine] = useState<PickLine | null>(null);

  useEffect(() => {
    document.body.style.background = "transparent";
    let stop: (() => void) | undefined;
    // `listen` はほかの窓あての知らせも受け取るので、この窓あての分だけを受け取る(画面ごとに位置が違うため)
    getCurrentWebviewWindow()
      .listen<PickLine | null>("pet://pick-target", (e) => setLine(e.payload))
      .then((u) => {
        stop = u;
      });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") api.petPickCancel().catch(() => undefined);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      stop?.();
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  return (
    <div
      className="pet-pick"
      onClick={() => api.petPickChoose().catch(() => undefined)}
      onContextMenu={(e) => {
        e.preventDefault();
        api.petPickCancel().catch(() => undefined);
      }}
    >
      <div className="pet-pick__hint">行き先をクリック(Escでやめる)</div>
      {line && (
        <div className="pet-pick__line" style={{ left: line.x, top: line.y - 4, width: line.w }}>
          <span className="pet-pick__label">{line.label}</span>
        </div>
      )}
    </div>
  );
}

export default PetPick;
