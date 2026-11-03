import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import PetView from "./components/pet/PetView";
import PetPanel from "./components/pet/PetPanel";
import PetBubble from "./components/pet/PetBubble";
import PetDebug from "./components/pet/PetDebug";
import { api } from "./api/tauri";
import "./styles/tokens.css";

// 画面側の未処理のエラーを、ログ(data/logs/)に残す(詩織Ver4.0)。
window.addEventListener("error", (e) => {
  api.logFrontendError(`${e.message} (${e.filename}:${e.lineno})`).catch(() => undefined);
});
window.addEventListener("unhandledrejection", (e) => {
  api.logFrontendError(`処理されなかったPromiseの失敗: ${String(e.reason)}`).catch(() => undefined);
});

// ペットの形の窓(詩織Ver6.0の段階4)は、同じ画面のコードを別の入口で読む(index.html?view=…)
const view = new URLSearchParams(window.location.search).get("view");
const Root =
  view === "pet"
    ? PetView
    : view === "pet-panel"
      ? PetPanel
      : view === "pet-bubble"
        ? PetBubble
        : view === "pet-debug"
          ? PetDebug
          : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
