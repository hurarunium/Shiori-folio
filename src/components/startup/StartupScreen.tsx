import { useEffect, useState } from "react";
import shioriMark from "../../assets/logo/shiori-mark-startup.png";
import { api, isServiceFailed, type ServiceStatus } from "../../api/tauri";
import { BackendStartupError, useShioriStore } from "../../store/useShioriStore";
import "./StartupScreen.css";

interface Props {
  onFinished: () => void;
}

// サービス名(Rust側のServiceStatus.name)をユーザー向けの日本語表示に変換する。
function serviceLabel(name: string): string {
  switch (name) {
    case "llm":
      return "会話用のAIモデル";
    case "embedding":
      return "検索用の埋め込みモデル";
    case "rag":
      return "詩織の図書館(検索インデックス)";
    default:
      return name;
  }
}

// 起動画面に常時表示する個別サービス一覧(2026-10-02追加)。LLM/embeddingが
// 並列起動するようになったことで、単一のstageLabelだけでは「今どのサービスが
// 終わっていて、どれがまだなのか」が分からなくなったため、サービスごとの
// 進捗を個別に見せる。表示順は起動処理の依存関係(llm/embeddingが並列→
// 両方完了後にragが続く)と対応させている。
const SERVICE_ORDER = ["llm", "embedding", "rag"] as const;

// 起動画面(2026-08-13新規実装)。当初はロゴの色づきを固定2.9秒のCSSアニメーション
// だけで演出していたが、実際のバックエンド起動(LLM/埋め込み/RAG検索サーバーの
// 起動〜モデル読み込み完了)には数秒〜十数秒かかるため、演出と実態が大きく
// ずれていた(同日、起動画面のタイミング問題の指示書で発覚)。
// ensureBackendServicesStarted()の完了を実際に待ち受けるよう修正: 完了するまでは
// 色づきを漸近的に(ASYMPTOTE_PERCENTの手前で足踏みするよう)進め続け、完了を
// 検知したら短時間で残りを埋めてから少し間を置いてフェードアウトする。
const ASYMPTOTE_PERCENT = 90;
const TIME_CONSTANT_MS = 4000;
const CATCH_UP_MS = 400;
const HOLD_MS = 400;
const FADE_MS = 500;

function StartupScreen({ onFinished }: Props) {
  const [percent, setPercent] = useState(0);
  const [ready, setReady] = useState(false);
  const [fadingOut, setFadingOut] = useState(false);
  // healthy:falseのまま返ってきたサービス(通信エラー等の「本当の例外」とは区別する)。
  // 空でない間はホーム画面へ進めず、エラー表示+再試行ボタンを出し続ける。
  const [failedServices, setFailedServices] = useState<ServiceStatus[] | null>(null);
  const [retryingName, setRetryingName] = useState<string | null>(null);
  // Rust側(start_backend_services_impl)が各段階の開始時に発行するshiori:startup-stage
  // イベントを、ストア側(useShioriStore.ensureBackendServicesStarted)でリスナー登録→
  // 起動呼び出しの順を保証した上で購読している(取りこぼし防止)。ここでは購読結果の
  // stateをそのまま表示するだけ。
  const stageLabel = useShioriStore((s) => s.startupStageLabel);
  // サービスごとの起動状況(shiori:service-statusイベント経由)。まだイベントが
  // 届いていないサービスはundefinedのままで「起動中」として表示する。
  const serviceStatuses = useShioriStore((s) => s.serviceStatuses);

  // バックエンド起動の完了を実際に待ち受ける。App.tsx側でも同じ関数を呼んでいるが、
  // モジュールスコープのPromiseで多重呼び出しを防いでいるため、実際のinvoke()は
  // 1回しか発生しない(どちらが先に呼んでも同じPromiseを共有する)。
  //
  // healthy:falseを含む結果はBackendStartupErrorとして投げられる(useShioriStore参照)。
  // これは「正常に返ってきたが起動できていない」ケースなので、握りつぶさずエラー表示+
  // 再試行ボタンを出す。それ以外の例外(IPC通信エラー等、ユーザー側で対処しようがない
  // 本当の想定外)は、従来通り起動画面に固まらないよう完了扱いにして先へ進む。
  useEffect(() => {
    let cancelled = false;
    useShioriStore
      .getState()
      .ensureBackendServicesStarted()
      .then(() => {
        if (!cancelled) setReady(true);
      })
      .catch((err) => {
        if (cancelled) return;
        if (err instanceof BackendStartupError) {
          setFailedServices(err.services.filter(isServiceFailed));
        } else {
          setReady(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleRetry = async (service: ServiceStatus) => {
    setRetryingName(service.name);
    try {
      if (service.name === "rag") {
        const result = await api.retryRagService();
        setFailedServices((prev) =>
          result.healthy ? (prev?.filter((s) => s.name !== "rag") ?? null) : (prev?.map((s) => (s.name === "rag" ? result : s)) ?? null),
        );
      } else {
        // llm/embeddingはセットで再起動される(restart_llm_services)。
        const results = await api.restartLlmServices();
        const stillUnhealthy = new Map(results.filter(isServiceFailed).map((r) => [r.name, r]));
        setFailedServices((prev) => {
          const others = prev?.filter((s) => s.name !== "llm" && s.name !== "embedding") ?? [];
          return [...others, ...stillUnhealthy.values()];
        });
      }
    } catch (err) {
      // 再試行自体の通信エラー等。エラー内容をそのサービスの表示に反映する。
      setFailedServices((prev) => prev?.map((s) => (s.name === service.name ? { ...s, error: String(err) } : s)) ?? null);
    } finally {
      setRetryingName(null);
    }
  };

  // 再試行によって全サービスが健全になったら、通常のフェードアウト経路へ合流する。
  useEffect(() => {
    if (failedServices !== null && failedServices.length === 0) {
      setFailedServices(null);
      setReady(true);
    }
  }, [failedServices]);

  // 完了通知が来るまで、色づきをASYMPTOTE_PERCENT手前まで漸近的に進める
  // (指数関数的に鈍化するので、実際の起動が長引いても唐突に止まって見えない)。
  useEffect(() => {
    if (ready) return;
    let rafId: number;
    const start = performance.now();
    const tick = (now: number) => {
      const elapsed = now - start;
      setPercent(ASYMPTOTE_PERCENT * (1 - Math.exp(-elapsed / TIME_CONSTANT_MS)));
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(rafId);
  }, [ready]);

  // 完了検知後: 残りを一気に埋め、少し間を置いてフェードアウトする
  useEffect(() => {
    if (!ready) return;
    setPercent(100);
    const fadeTimer = setTimeout(() => setFadingOut(true), CATCH_UP_MS + HOLD_MS);
    const doneTimer = setTimeout(onFinished, CATCH_UP_MS + HOLD_MS + FADE_MS);
    return () => {
      clearTimeout(fadeTimer);
      clearTimeout(doneTimer);
    };
  }, [ready, onFinished]);

  return (
    <div className={`startup-screen${fadingOut ? " startup-screen--fade-out" : ""}`}>
      <div className="startup-screen__mark-wrap">
        <img
          src={shioriMark}
          className="startup-screen__mark startup-screen__mark--gray"
          alt=""
          aria-hidden="true"
        />
        <img
          src={shioriMark}
          className={`startup-screen__mark startup-screen__mark--color${ready ? " startup-screen__mark--color--catchup" : ""}`}
          alt="詩織"
          style={{ clipPath: `inset(${100 - percent}% 0 0 0)` }}
        />
      </div>
      <div className="startup-screen__stage-label">{stageLabel}</div>

      <div className="startup-screen__service-list">
        {SERVICE_ORDER.map((name) => {
          const status = serviceStatuses[name];
          const state = !status ? "pending" : status.skipped ? "skipped" : status.healthy ? "done" : "failed";
          return (
            <div className="startup-screen__service-row" key={name} data-state={state}>
              <span className="startup-screen__service-dot" aria-hidden="true" />
              <span className="startup-screen__service-name">
                {serviceLabel(name)}
                {state === "skipped" && (name === "llm" ? "(外部AIモードでは使いません)" : "(サーバーが担います)")}
              </span>
            </div>
          );
        })}
      </div>

      {failedServices && failedServices.length > 0 && (
        <div className="startup-screen__error-panel">
          <div className="startup-screen__error-title">
            起動できていないサービスがあります。会話や検索が正しく動かない可能性があります。
          </div>
          {failedServices.map((service) => (
            <div className="startup-screen__error-row" key={service.name}>
              <div className="startup-screen__error-service">
                <span>{serviceLabel(service.name)}</span>
                {service.error && <span className="startup-screen__error-detail">{service.error}</span>}
              </div>
              <button
                className="startup-screen__retry-btn"
                onClick={() => handleRetry(service)}
                disabled={retryingName !== null}
              >
                {retryingName === service.name ? "再試行中..." : "再試行"}
              </button>
            </div>
          ))}
          {/* 会話用のLLMだけが起動できないときは、会話なしで先へ進める(検索・図書館・タスクなどは、LLMを使わない) */}
          {failedServices.every((s) => s.name === "llm") && (
            <button
              className="startup-screen__retry-btn"
              onClick={() => setFailedServices([])}
              disabled={retryingName !== null}
            >
              会話なしで続ける
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export default StartupScreen;
