import { useEffect } from "react";
import { useNoticeStore } from "../../store/useNoticeStore";
import { useMainViewStore } from "../../store/useMainViewStore";
import { usePendingStore } from "../../store/usePendingStore";
import { useReduceMotion } from "../../store/useMotionStore";
import { api } from "../../api/tauri";
import type { Notice, NoticeKind } from "../../types";
import "./Notices.css";

// 知らせの札(詩織Ver6.0の段階3)。上の帯の下、画面の右上に、新しいものを上に積む。
// 押すと、その知らせの元へ移る(タイマーと拡張機能は小道具の枠、Claudeは作業が終わったセッションのアプリ、
// 要確認は一覧)。Claudeのアプリを前に出せなかったとき(別のPCのセッションなど)はAIの画面のClaudeへ。

const KIND_LABEL: Record<NoticeKind, string> = {
  timer: "タイマー",
  claude: "Claude",
  pending: "要確認",
  extension: "知らせ",
};

export function openSource(n: Notice) {
  const view = useMainViewStore.getState();
  if (n.kind === "claude") view.openClaude();
  else if (n.kind === "pending") usePendingStore.getState().setOpen(true);
  else if (n.source) view.showTool(n.source);
}

// 札を押したとき。Claudeは先にセッションのアプリを前に出してみる(ペットの吹き出しは本体の側で同じことをする)
function openFromCard(n: Notice) {
  if (n.kind === "claude" && n.source) {
    api
      .openClaudeSession(n.source)
      .then((ok) => {
        if (!ok) openSource(n);
      })
      .catch(() => openSource(n));
    return;
  }
  openSource(n);
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function Notices() {
  const notices = useNoticeStore((s) => s.notices);
  const dismiss = useNoticeStore((s) => s.dismiss);
  const reduce = useReduceMotion();

  useEffect(() => {
    useNoticeStore.getState().load();
    let stop: (() => void) | undefined;
    useNoticeStore
      .getState()
      .subscribe()
      .then((unlisten) => {
        stop = unlisten;
      });
    return () => stop?.();
  }, []);

  if (notices.length === 0) return null;
  return (
    <div className={`notices${reduce ? " notices--still" : ""}`} aria-live="polite">
      {notices.map((n) => (
        <div key={n.id} className="notices__card" data-notice-kind={n.kind} data-notice-id={n.id}>
          <button
            type="button"
            className="notices__main"
            data-ripple
            onClick={() => {
              openFromCard(n);
              dismiss(n.id);
            }}
          >
            <span className="notices__head">
              <span className="notices__kind">{KIND_LABEL[n.kind]}</span>
              <span className="notices__time">{formatTime(n.at)}</span>
            </span>
            <span className="notices__title">{n.title}</span>
            {n.body && <span className="notices__body">{n.body}</span>}
          </button>
          <button type="button" className="notices__close" onClick={() => dismiss(n.id)} aria-label="知らせを閉じる">
            ×
          </button>
        </div>
      ))}
    </div>
  );
}

export default Notices;
