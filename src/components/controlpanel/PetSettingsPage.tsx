import { useEffect, useState } from "react";
import { api } from "../../api/tauri";
import { NumberRow, TextRow } from "./shared";
import { PetBehaviorPanel } from "./PetBehaviorPanel";

// ペットの形の設定(詩織Ver6.0の段階4)。形の切り替えのホットキー、眠るまでの時間、本と小道具の枠の大きさ。
// 保存するとconfig.jsonのpetに書く。ホットキーの変更は、録音のホットキーと同じく次の起動から。
// 本と小道具の枠の大きさは、次に本を開いたときから変わる。
// その下に、ペットの行動の設定(PetBehaviorPanel。ペットの行動の設計書の6節)を置く。保存は別。
interface PetForm {
  hotkey: string;
  sleep: number;
  panel: number;
}

export function PetSettingsPage() {
  const [original, setOriginal] = useState<PetForm | null>(null);
  const [form, setForm] = useState<PetForm | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getConfig()
      .then((c) => {
        const v = { hotkey: c.petHotkey, sleep: c.petSleepMinutes, panel: c.petPanelPercent };
        setOriginal(v);
        setForm(v);
      })
      .catch((e) => setError(String(e)));
  }, []);

  if (!form || !original) {
    return (
      <div className="control-panel__page-head">
        <h1>ペット</h1>
        <p>{error ?? "読み込み中..."}</p>
      </div>
    );
  }

  const save = async () => {
    setError(null);
    setMessage(null);
    try {
      await api.setConfig({
        ...(form.hotkey !== original.hotkey && { petHotkey: form.hotkey }),
        ...(form.sleep !== original.sleep && { petSleepMinutes: form.sleep }),
        ...(form.panel !== original.panel && { petPanelPercent: form.panel }),
      });
      setMessage(
        form.hotkey !== original.hotkey
          ? "保存しました。ホットキーは次の起動から変わります。"
          : "保存しました。眠るまでの時間は次にペットの形にしたときから、枠の大きさは次に本を開いたときから変わります。",
      );
      setOriginal(form);
    } catch (e) {
      setError(String(e));
    }
  };

  const changed = form.hotkey !== original.hotkey || form.sleep !== original.sleep || form.panel !== original.panel;
  return (
    <>
      <div className="control-panel__page-head">
        <h1>ペット</h1>
        <p>ペットの形は、上の帯の「ペット」かホットキーで切り替えます。戻るときは、ペットを押して開く本の「ウィンドウに戻る」かホットキーです。</p>
      </div>
      {error && <div className="control-panel__error">{error}</div>}
      <div className="control-panel__panel" data-pet-settings>
        <div className="control-panel__panel-label">ペットの形</div>
        <TextRow label="形の切り替えのホットキー" value={form.hotkey} onChange={(v) => setForm({ ...form, hotkey: v })} />
        <NumberRow label="眠るまでの時間(分、1〜120)" value={form.sleep} onChange={(v) => setForm({ ...form, sleep: v })} />
        <NumberRow
          label="本と小道具の枠の大きさ(%、60〜150)"
          value={form.panel}
          onChange={(v) => setForm({ ...form, panel: v })}
        />
        <div className="control-panel__field-row">
          <span className="control-panel__field-label">{message ?? ""}</span>
          <button type="button" className="control-panel__btn" disabled={!changed} onClick={save}>
            保存
          </button>
        </div>
      </div>
      <PetBehaviorPanel />
    </>
  );
}
