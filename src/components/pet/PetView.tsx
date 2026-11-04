import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api/tauri";
import type { PetFrame } from "../../types";
import { EXT_ORIGIN } from "../extensions/ExtensionFrame";
import "./pet.css";

// ペットのウィンドウの中身(詩織Ver6.0の段階4)。位置・状態・絵のコマは本体(pet/mod.rs)が決めて
// `pet://frame` で送るので、ここは絵を出すだけ。ウィンドウはキャラクターの絵と同じ大きさ(段階0の方式A)。
// 動きの絵がまだ無い状態は、動きを小さな印と揺れで表す(状態ごとのクラス。本当の動きの絵に差し替えたら外す)。
// 歩く以外は本当の絵がある(現れる・反応する・喜ぶ・つままれる・着地は、仮の揺れも重ねたまま残す。ふらる)。眠っている間の「Z」は、絵ではなく演出として残す。
function PetView() {
  const [frame, setFrame] = useState<PetFrame | null>(null);

  useEffect(() => {
    let stop: (() => void) | undefined;
    listen<PetFrame>("pet://frame", (e) => setFrame(e.payload)).then((u) => {
      stop = u;
    });
    return () => stop?.();
  }, []);

  if (!frame) return null;
  const position = frame.frames > 1 ? `${(frame.frame / (frame.frames - 1)) * 100}% 0` : "0 0";
  return (
    <div
      className={`pet pet--${frame.state} pet--${frame.activity}`}
      data-pet-state={frame.state}
      onMouseDown={(e) => {
        // Macのcontrol+クリックは右クリックとして扱う
        if (e.button === 0 && !e.ctrlKey) api.petPress().catch(() => undefined);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        api.petOpenMenu().catch(() => undefined);
      }}
    >
      <div
        className="pet__sprite"
        style={{
          backgroundImage: `url(${EXT_ORIGIN}/_shiori/pet/${encodeURIComponent(frame.file)})`,
          backgroundSize: `${frame.frames * 100}% 100%`,
          backgroundPosition: position,
          transform: frame.facing < 0 ? "scaleX(-1)" : undefined,
        }}
      />
      {frame.state === "sleep" && (
        // 眠っている間の「Z」(絵ではなく演出)。頭の上から、ずらして3つ浮かべる
        <span className={`pet__zzz${frame.facing < 0 ? " pet__zzz--left" : ""}`} aria-hidden>
          <span>z</span>
          <span>z</span>
          <span>Z</span>
        </span>
      )}
      {(frame.state === "notify" || frame.state === "happy") && (
        <span className={`pet__glow${frame.facing < 0 ? " pet__glow--left" : ""}`} />
      )}
    </div>
  );
}

export default PetView;
