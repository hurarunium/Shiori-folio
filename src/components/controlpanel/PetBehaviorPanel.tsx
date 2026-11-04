import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../api/tauri";
import type {
  PetAction,
  PetAppPref,
  PetBehavior,
  PetBehaviorDto,
  PetLike,
  PetMode,
  PetModeValues,
  PetSeenApp,
} from "../../types";
import { ToggleRow } from "./shared";

// ペットの行動の設定(ペットの行動の設計書の6節、企画書のP-10・P-11)。行動アルゴリズムの形は固定で、
// ここではパラメーター(ウィンドウに乗るか・活発さ・行動の重み・邪魔しない条件・アプリの好み)を変える。
// 保存すると data/pet/behavior.json に書き、ペットの形のままでもすぐ効く。

const ACTIONS: { key: PetAction; label: string }[] = [
  { key: "stand", label: "立つ" },
  { key: "sit", label: "縁に座る" },
  { key: "read", label: "本を読む" },
  { key: "walk", label: "歩く" },
  { key: "jump", label: "飛び移る" },
  { key: "screen", label: "画面を移る" },
  { key: "hang", label: "ぶら下がる" },
  { key: "climb", label: "登る" },
  { key: "sleep", label: "うたた寝" },
];

const LIKES: { key: PetLike; label: string }[] = [
  { key: "avoid", label: "避ける(乗らない)" },
  { key: "normal", label: "普通" },
  { key: "like", label: "好き" },
  { key: "love", label: "大好き" },
];

const PRESETS: { key: keyof PetBehaviorDto["presets"]; label: string }[] = [
  { key: "quiet", label: "静か" },
  { key: "normal", label: "普通" },
  { key: "play", label: "遊ぶ" },
];

const MODES: { key: PetMode; label: string }[] = [
  { key: "normal", label: "普段" },
  { key: "work", label: "作業中" },
];

const OS_LABEL: Record<PetSeenApp["os"], string> = { mac: "Mac", win: "Windows" };

// 0〜maxの整数を選ぶ行(重みと活発さ)
function RangeRow({
  label,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  return (
    <div className="control-panel__slider-row">
      <div className="control-panel__slider-top">
        <span>{label}</span>
        <span className="control-panel__slider-val">{value}</span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={1}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        className="control-panel__slider-input"
      />
    </div>
  );
}

export function PetBehaviorPanel() {
  const [presets, setPresets] = useState<PetBehaviorDto["presets"] | null>(null);
  const [original, setOriginal] = useState<PetBehavior | null>(null);
  const [form, setForm] = useState<PetBehavior | null>(null);
  // 活発さと行動の重みを、どちらのモードの分として見ているか
  const [tab, setTab] = useState<PetMode>("normal");
  const [seen, setSeen] = useState<PetSeenApp[]>([]);
  const [newName, setNewName] = useState("");
  const [pick, setPick] = useState("");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .petBehaviorGet()
      .then((d) => {
        setPresets(d.presets);
        setOriginal(d.behavior);
        setForm(d.behavior);
      })
      .catch((e) => setError(String(e)));
    const loadSeen = () =>
      api
        .petSeenApps()
        .then(setSeen)
        .catch(() => undefined);
    loadSeen();
    // 見かけたアプリはペットの形のあいだに増えるので、ウィンドウの形に戻ったら読み直す
    // (この画面は、ペットの形のあいだも開いたまま残るため)
    let stop: (() => void) | undefined;
    listen<string>("pet://mode", (e) => {
      if (e.payload === "window") loadSeen();
    }).then((u) => {
      stop = u;
    });
    return () => stop?.();
  }, []);

  // 登録済みの名前(大文字・小文字は区別しない)
  const registered = useMemo(
    () => new Set((form?.apps ?? []).flatMap((a) => a.names.map((n) => n.toLowerCase()))),
    [form],
  );
  const unregistered = seen.filter((s) => !registered.has(s.name.toLowerCase()));

  if (!form || !original || !presets) {
    return (
      <div className="control-panel__panel">
        <div className="control-panel__panel-label">ペットの行動</div>
        <p>{error ?? "読み込み中..."}</p>
      </div>
    );
  }

  const update = (f: PetBehavior) => {
    setForm(f);
    setMessage(null);
  };
  const setApps = (apps: PetAppPref[]) => update({ ...form, apps });
  const setApp = (i: number, a: PetAppPref) => setApps(form.apps.map((x, j) => (j === i ? a : x)));

  const values: PetModeValues = tab === "work" ? form.work : { activity: form.activity, weights: form.weights };
  const setValues = (v: PetModeValues) => update(tab === "work" ? { ...form, work: v } : { ...form, ...v });

  // ひな形は、見ているモードの活発さ・重みを入れ替える。普段のときは邪魔しない条件も入れ替える
  // (邪魔しない条件はモードで分けない。ウィンドウに乗るかと、アプリの好みはそのまま)
  const applyPreset = (key: keyof PetBehaviorDto["presets"]) => {
    const p = presets[key];
    if (tab === "work") {
      setValues({ activity: p.activity, weights: p.weights });
      return;
    }
    update({
      ...form,
      activity: p.activity,
      weights: p.weights,
      quietOnFocused: p.quietOnFocused,
      noRideFullscreen: p.noRideFullscreen,
    });
  };

  const addApp = (name: string) => {
    const n = name.trim();
    if (!n || registered.has(n.toLowerCase())) return;
    setApps([...form.apps, { names: [n], like: "normal", favorite: null, work: false }]);
  };

  const removeName = (i: number, name: string) => {
    const names = form.apps[i].names.filter((n) => n !== name);
    if (names.length === 0) setApps(form.apps.filter((_, j) => j !== i));
    else setApp(i, { ...form.apps[i], names });
  };

  const save = async () => {
    setError(null);
    setMessage(null);
    try {
      const saved = await api.petBehaviorSet(form);
      setOriginal(saved);
      setForm(saved);
      setMessage("保存しました。ペットの形のままでもすぐ効きます。");
    } catch (e) {
      setError(String(e));
    }
  };

  const changed = JSON.stringify(form) !== JSON.stringify(original);
  const seenLabel = (s: PetSeenApp) => `${s.name}(${OS_LABEL[s.os]})`;

  return (
    <>
      {error && <div className="control-panel__error">{error}</div>}
      <div className="control-panel__panel" data-pet-behavior>
        <div className="control-panel__panel-label">ペットの行動</div>
        <ToggleRow
          label="ウィンドウに乗る(オフなら床の上だけ)"
          checked={form.enabled}
          onChange={(v) => update({ ...form, enabled: v })}
        />
        <div className="control-panel__field-row">
          <span className="control-panel__field-label">調整するモード</span>
          <div className="control-panel__actions">
            {MODES.map((m) => (
              <button
                key={m.key}
                type="button"
                className="control-panel__btn"
                disabled={tab === m.key}
                aria-pressed={tab === m.key}
                onClick={() => setTab(m.key)}
              >
                {m.label}
              </button>
            ))}
          </div>
        </div>
        <p className="control-panel__pet-note">
          作業アプリが前面に来ると作業中のモードに入り、使っていない画面か端へ移ります。前面でなくなって5分で普段に戻ります。ペットの右クリックで固定もできます。
        </p>
        <RangeRow
          label="活発さ(1: 静か 〜 5: 活発)"
          value={values.activity}
          min={1}
          max={5}
          onChange={(v) => setValues({ ...values, activity: v })}
        />
        <div className="control-panel__field-row">
          <span className="control-panel__field-label">ひな形から始める</span>
          <div className="control-panel__actions">
            {PRESETS.map((p) => (
              <button key={p.key} type="button" className="control-panel__btn" onClick={() => applyPreset(p.key)}>
                {p.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">行動のしやすさ(0ならその行動をしない)</div>
        {ACTIONS.map((a) => (
          <RangeRow
            key={a.key}
            label={a.label}
            value={values.weights[a.key]}
            min={0}
            max={10}
            onChange={(v) => setValues({ ...values, weights: { ...values.weights, [a.key]: v } })}
          />
        ))}
      </div>

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">作業の邪魔をしない</div>
        <p className="control-panel__pet-note">
          クリックを吸わないことと、ウィンドウの操作(ドラッグ・大きさの変更・上辺のボタン)を妨げないことは、いつも守ります。
        </p>
        <ToggleRow
          label="入力中のウィンドウの上では静かにする"
          checked={form.quietOnFocused}
          onChange={(v) => update({ ...form, quietOnFocused: v })}
        />
        <ToggleRow
          label="全画面のアプリが前面のときは、その画面のウィンドウに乗らない"
          checked={form.noRideFullscreen}
          onChange={(v) => update({ ...form, noRideFullscreen: v })}
        />
      </div>

      <div className="control-panel__panel">
        <div className="control-panel__panel-label">アプリの好み</div>
        <p className="control-panel__pet-note">
          好きなアプリには行きやすく、長くいます。避けるアプリには乗りません。同じアプリでもMacとWindowsで名前が違うときは、1つに両方の名前を足せます。
        </p>
        {form.apps.length === 0 && <p className="control-panel__pet-note">まだ登録がありません(全部「普通」)。</p>}
        {form.apps.map((a, i) => (
          <div key={i} className="control-panel__pet-app">
            <div className="control-panel__pet-names">
              {a.names.map((n) => (
                <span key={n} className="control-panel__pet-chip">
                  {n}
                  <button type="button" aria-label={`${n}を外す`} onClick={() => removeName(i, n)}>
                    ×
                  </button>
                </span>
              ))}
              {unregistered.length > 0 && (
                <select
                  className="control-panel__text-input"
                  value=""
                  onChange={(e) => e.target.value && setApp(i, { ...a, names: [...a.names, e.target.value] })}
                >
                  <option value="">別の名前を足す…</option>
                  {unregistered.map((s) => (
                    <option key={`${s.os}:${s.name}`} value={s.name}>
                      {seenLabel(s)}
                    </option>
                  ))}
                </select>
              )}
            </div>
            <div className="control-panel__pet-app-row">
              <select
                className="control-panel__text-input"
                value={a.like}
                onChange={(e) => setApp(i, { ...a, like: e.target.value as PetLike })}
              >
                {LIKES.map((l) => (
                  <option key={l.key} value={l.key}>
                    {l.label}
                  </option>
                ))}
              </select>
              <select
                className="control-panel__text-input"
                value={a.favorite ?? ""}
                disabled={a.like === "avoid"}
                onChange={(e) => setApp(i, { ...a, favorite: (e.target.value || null) as PetAction | null })}
              >
                <option value="">好きな行動なし</option>
                {ACTIONS.map((x) => (
                  <option key={x.key} value={x.key}>
                    {x.label}しやすい
                  </option>
                ))}
              </select>
              <button type="button" className="control-panel__btn" onClick={() => setApps(form.apps.filter((_, j) => j !== i))}>
                消す
              </button>
            </div>
            <ToggleRow
              label="作業アプリ(前面に来ると作業中のモードに入る)"
              checked={a.work}
              onChange={(v) => setApp(i, { ...a, work: v })}
            />
          </div>
        ))}
        <div className="control-panel__field-row">
          <span className="control-panel__field-label">見かけたアプリから選ぶ</span>
          <div className="control-panel__actions">
            <select className="control-panel__text-input" value={pick} onChange={(e) => setPick(e.target.value)}>
              <option value="">{unregistered.length === 0 ? "(まだありません)" : "選ぶ…"}</option>
              {unregistered.map((s) => (
                <option key={`${s.os}:${s.name}`} value={s.name}>
                  {seenLabel(s)}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="control-panel__btn"
              disabled={!pick}
              onClick={() => {
                addApp(pick);
                setPick("");
              }}
            >
              登録
            </button>
          </div>
        </div>
        <div className="control-panel__field-row">
          <span className="control-panel__field-label">名前を入力して足す</span>
          <div className="control-panel__actions">
            <input
              type="text"
              className="control-panel__text-input"
              value={newName}
              placeholder="例: Google Chrome"
              onChange={(e) => setNewName(e.target.value)}
            />
            <button
              type="button"
              className="control-panel__btn"
              disabled={!newName.trim() || registered.has(newName.trim().toLowerCase())}
              onClick={() => {
                addApp(newName);
                setNewName("");
              }}
            >
              足す
            </button>
          </div>
        </div>
        <p className="control-panel__pet-note">見かけたアプリは、ペットの形のときに開いていたウィンドウから自動で並びます。</p>
      </div>

      <div className="control-panel__field-row">
        <span className="control-panel__field-label">{message ?? ""}</span>
        <button type="button" className="control-panel__btn" disabled={!changed} onClick={save}>
          行動の設定を保存
        </button>
      </div>
    </>
  );
}
