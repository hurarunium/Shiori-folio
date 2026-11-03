import { useEffect, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

// 足場の確かめの表示(ペットの行動の設計書のPhase 1)。`SHIORI_PET_DEBUG=1` で起動したときだけ、
// 画面ごとに透明でクリックを通すウィンドウを開き、本体(pet/mod.rs)が計算した足場に線を引く。
// 床は青、ウィンドウの上辺は赤。線の上にアプリ名を出す。
interface DebugLine {
  x: number;
  y: number;
  w: number;
  label: string;
  floor: boolean;
}

function PetDebug() {
  const [lines, setLines] = useState<DebugLine[]>([]);

  useEffect(() => {
    document.body.style.background = "transparent";
    let stop: (() => void) | undefined;
    // `listen` はほかの窓あての知らせも受け取るので、この窓あての分だけを受け取る(画面ごとに中身が違うため)
    getCurrentWebviewWindow()
      .listen<DebugLine[]>("pet://debug", (e) => setLines(e.payload))
      .then((u) => {
        stop = u;
      });
    return () => stop?.();
  }, []);

  return (
    <div style={{ position: "fixed", inset: 0, pointerEvents: "none" }}>
      {lines.map((l, i) => {
        const color = l.floor ? "rgba(40, 120, 255, 0.9)" : "rgba(255, 40, 40, 0.9)";
        return (
          <div key={i} style={{ position: "absolute", left: l.x, top: l.y - 3, width: l.w, height: 3, background: color }}>
            <span
              style={{
                position: "absolute",
                left: 4,
                bottom: 4,
                font: "12px sans-serif",
                color: "#fff",
                background: color,
                padding: "0 4px",
                whiteSpace: "nowrap",
              }}
            >
              {l.label}
            </span>
          </div>
        );
      })}
    </div>
  );
}

export default PetDebug;
