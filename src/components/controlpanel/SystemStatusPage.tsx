import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import type { SystemInfo } from "../../types";
import { Sparkline, formatMb, levelFor, levelForTemp } from "./shared";

// システム状態ページ(優先度1、読み取り専用)。ポーリングで得たSystemInfoと
// 推移履歴はControlPanel.tsx側で保持し、ここには表示用途としてのみ渡す
// (ModelSwitchPageのplatform判定でも同じSystemInfoを使うため、所有元は
// ControlPanel.tsxに残している)。
export function SystemStatusPage({
  info,
  error,
  vramPercent,
  ramPercent,
  vramHistory,
  ramHistory,
  tempHistory,
}: {
  info: SystemInfo | null;
  error: string | null;
  vramPercent: number | null;
  ramPercent: number | null;
  vramHistory: number[];
  ramHistory: number[];
  tempHistory: number[];
}) {
  const cpuPercent = info ? Math.round(info.cpuUsagePercent) : null;
  const tempLevel = info?.gpu?.temperatureC !== null && info?.gpu?.temperatureC !== undefined
    ? levelForTemp(info.gpu.temperatureC)
    : "safe";
  // Mac(Apple Silicon)にはdiscrete VRAMが存在しない(統合メモリ)ため、
  // VRAM・GPU温度ゲージ自体を表示せず、システムRAMのゲージに統合メモリである旨を添える。
  const isMac = info?.platform === "macos";
  // tauri.conf.jsonのversion(詩織Ver4.0で4.0.0にそろえ、Ver4.0.1で4.0.1、Ver4.1で4.1.0、Ver4.2で4.2.0、Ver6.0で6.0.0、Ver6.0.1で6.0.1、Ver6.1で6.1.0、Ver6.2で6.2.0に上げた)。
  const [appVersion, setAppVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => undefined);
  }, []);

  return (
    <>
      <div className="control-panel__page-head">
        <h1>システム状態</h1>
        <p>読み取り専用です。ここでは何も変更されません。</p>
        {appVersion && <p className="control-panel__version">詩織 Ver{appVersion}</p>}
      </div>

      {error && <div className="control-panel__error">取得に失敗しました: {error}</div>}

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">リソース使用状況</div>
        <div className="control-panel__gauge-row">
          {!isMac && (
            <div className="control-panel__gauge">
              <div className="control-panel__gauge-top">
                <span className="control-panel__gauge-name">VRAM</span>
                {vramPercent !== null && (
                  <span className={`control-panel__gauge-val control-panel__gauge-val--${levelFor(vramPercent)}`}>
                    {vramPercent}%
                  </span>
                )}
              </div>
              {info?.gpu ? (
                <>
                  <div className="control-panel__gauge-bar">
                    <div
                      className={`control-panel__gauge-fill control-panel__gauge-fill--${levelFor(vramPercent ?? 0)}`}
                      style={{ width: `${vramPercent}%` }}
                    />
                  </div>
                  <div className="control-panel__gauge-sub">
                    {formatMb(info.gpu.vramUsedMb)} / {formatMb(info.gpu.vramTotalMb)}
                  </div>
                  <div className={`control-panel__gauge-val--${levelFor(vramPercent ?? 0)}`}>
                    <Sparkline values={vramHistory} max={100} />
                  </div>
                </>
              ) : (
                <div className="control-panel__gauge-sub">GPU情報を取得できません(nvidia-smi未検出)</div>
              )}
            </div>
          )}

          <div className="control-panel__gauge">
            <div className="control-panel__gauge-top">
              <span className="control-panel__gauge-name">{isMac ? "統合メモリ" : "システムRAM"}</span>
              {ramPercent !== null && (
                <span className={`control-panel__gauge-val control-panel__gauge-val--${levelFor(ramPercent)}`}>
                  {ramPercent}%
                </span>
              )}
            </div>
            <div className="control-panel__gauge-bar">
              <div
                className={`control-panel__gauge-fill control-panel__gauge-fill--${levelFor(ramPercent ?? 0)}`}
                style={{ width: `${ramPercent ?? 0}%` }}
              />
            </div>
            <div className="control-panel__gauge-sub">
              {info ? `${formatMb(info.ramUsedMb)} / ${formatMb(info.ramTotalMb)}` : "取得中..."}
              {isMac && "(GPUと共有)"}
            </div>
            <div className={`control-panel__gauge-val--${levelFor(ramPercent ?? 0)}`}>
              <Sparkline values={ramHistory} max={100} />
            </div>
          </div>

          <div className="control-panel__gauge">
            <div className="control-panel__gauge-top">
              <span className="control-panel__gauge-name">CPU</span>
              {cpuPercent !== null && (
                <span className={`control-panel__gauge-val control-panel__gauge-val--${levelFor(cpuPercent)}`}>
                  {cpuPercent}%
                </span>
              )}
            </div>
            <div className="control-panel__gauge-bar">
              <div
                className={`control-panel__gauge-fill control-panel__gauge-fill--${levelFor(cpuPercent ?? 0)}`}
                style={{ width: `${cpuPercent ?? 0}%` }}
              />
            </div>
            <div className="control-panel__gauge-sub">{info?.cpu.model ?? "取得中..."}</div>
          </div>

          {!isMac && (
            <div className="control-panel__gauge">
              <div className="control-panel__gauge-top">
                <span className="control-panel__gauge-name">GPU温度</span>
                {info?.gpu?.temperatureC != null && (
                  <span className={`control-panel__gauge-val control-panel__gauge-val--${tempLevel}`}>
                    {info.gpu.temperatureC}℃
                  </span>
                )}
              </div>
              <div className="control-panel__gauge-sub">
                {info?.gpu?.temperatureC == null ? "取得できません" : "80℃以上で注意"}
              </div>
              <div className={`control-panel__gauge-val--${tempLevel}`}>
                <Sparkline values={tempHistory} max={100} />
              </div>
            </div>
          )}
        </div>

        {info?.diskThroughput && (
          <div className="control-panel__disk-io">
            ディスク I/O(
            {info.storage?.drive ?? "動作ドライブ"}): 読み込み {info.diskThroughput.readMbPerSec.toFixed(1)}MB/s ／
            書き込み {info.diskThroughput.writeMbPerSec.toFixed(1)}MB/s
          </div>
        )}
      </div>

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">サービス稼働状況</div>
        <div className="control-panel__service-list">
          {(info?.services ?? []).map((service) => (
            <div className="control-panel__service-item" key={service.name}>
              <span>
                {service.name}
                {service.modelName && (
                  <span className="control-panel__service-model"> · {service.modelName}</span>
                )}
              </span>
              <span className="control-panel__service-status">
                {service.running && <span className="control-panel__pulse" />}
                {service.running ? "稼働中" : "待機(オンデマンド)"}
                <span className="control-panel__service-usage">
                  VRAM {service.vramMb !== null ? formatMb(service.vramMb) : "—"} / RAM{" "}
                  {service.ramMb !== null ? formatMb(service.ramMb) : "—"}
                </span>
              </span>
            </div>
          ))}
        </div>
      </div>

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">PC本体情報</div>
        <div className="control-panel__pc-info">
          <div>OS：{info?.os ?? "取得中..."}</div>
          <div>
            CPU：{info?.cpu.model ?? "取得中..."}
            {info && ` (物理${info.cpu.physicalCores}コア／論理${info.cpu.logicalCores}スレッド／${(info.cpu.frequencyMhz / 1000).toFixed(1)}GHz)`}
          </div>
          {info?.gpu && <div>GPU：{info.gpu.name}</div>}

          {info?.storage && (
            <div>
              ストレージ：{info.storage.drive}（{info.storage.kind}
              {info.storage.isRemovable ? "・リムーバブル(USB等)" : ""}／{info.storage.fileSystem}） 空き{" "}
              {info.storage.freeGb.toFixed(1)}GB / 総容量 {info.storage.totalGb.toFixed(1)}GB
            </div>
          )}

          {info?.power && (
            <div>
              電源：
              {info.power.onBattery
                ? `バッテリー駆動${info.power.batteryPercent !== null ? `(残量${info.power.batteryPercent.toFixed(0)}%)` : ""}`
                : "AC電源接続中"}
            </div>
          )}
        </div>
      </div>
    </>
  );
}
