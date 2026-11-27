import { useAppModeStore } from "../store/useAppModeStore";
import { useShioriStore } from "../store/useShioriStore";
import "./ExternalModeNotice.css";

// 外部AIモードのときに、左の会話エリアの代わりに出す案内(詩織Ver4.0)。
// 会話モードへ戻した直後は、会話用LLMの起動が終わるまでここで待ってもらう。
function ExternalModeNotice() {
  const mode = useAppModeStore((s) => s.mode);
  const switching = useAppModeStore((s) => s.switching);
  const error = useAppModeStore((s) => s.error);
  const change = useAppModeStore((s) => s.change);
  const llm = useShioriStore((s) => s.serviceStatuses["llm"]);

  const startingConversation = mode === "conversation" && switching;

  return (
    <div className="external-mode-notice">
      <p className="external-mode-notice__title">
        {startingConversation ? "会話用のAIを起動しています…" : "外部AIモード"}
      </p>
      {!startingConversation && (
        <p className="external-mode-notice__body">
          会話用のAIを止めて、Claudeなど外部のAIと使うためのモードです。詩織との会話と音声入力は使えませんが、
          図書館・全件閲覧・AIの画面(稼働状況・利用統計)と、Claudeからの検索(MCP)はそのまま使えます。
        </p>
      )}
      {startingConversation && (
        <p className="external-mode-notice__body">モデルの読み込みに十数秒〜数十秒かかります。</p>
      )}
      {error && <p className="external-mode-notice__error">{error}</p>}
      {llm?.error && !switching && <p className="external-mode-notice__error">{llm.error}</p>}
      <button
        type="button"
        className="external-mode-notice__button"
        disabled={switching}
        onClick={() => change("conversation")}
      >
        {switching ? "切り替えています…" : "会話モードに戻す"}
      </button>
    </div>
  );
}

export default ExternalModeNotice;
