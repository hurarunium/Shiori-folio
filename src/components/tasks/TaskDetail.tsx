import { useEffect, useState, type FormEvent } from "react";
import { handleArticleLinkClick, renderMarkdown } from "../../lib/markdown";
import type { Task } from "../../types";
import "../library/ArticleDetail.css";
import "../main/PendingModal.css";

export interface TaskFormValues {
  summary: string;
  folder: string;
  due: string | null;
  detail: string;
}

interface FormProps {
  title: string;
  initial: TaskFormValues;
  folders: string[];
  busy: boolean;
  onSave: (values: TaskFormValues) => void;
  onCancel: () => void;
}

// 追加と編集の欄。右の列がそのまま入力欄になる。概要が空なら保存しない。
export function TaskForm({ title, initial, folders, busy, onSave, onCancel }: FormProps) {
  const [values, setValues] = useState(initial);
  const set = (change: Partial<TaskFormValues>) => setValues((v) => ({ ...v, ...change }));

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (values.summary.trim()) onSave(values);
  };

  return (
    <form className="tasks-form" onSubmit={submit}>
      <h2 className="tasks-detail__title">{title}</h2>
      <label className="tasks-form__field">
        <span>概要</span>
        <input
          autoFocus
          className="tasks-input"
          value={values.summary}
          onChange={(e) => set({ summary: e.target.value })}
        />
      </label>
      <label className="tasks-form__field">
        <span>フォルダ</span>
        <select className="tasks-input" value={values.folder} onChange={(e) => set({ folder: e.target.value })}>
          <option value="">未分類</option>
          {folders.map((f) => (
            <option key={f} value={f}>
              {f}
            </option>
          ))}
        </select>
      </label>
      <label className="tasks-form__field">
        <span>締切</span>
        <span className="tasks-form__due">
          <input
            type="date"
            className="tasks-input"
            value={values.due ?? ""}
            onChange={(e) => set({ due: e.target.value || null })}
          />
          {values.due && (
            <button type="button" className="tasks-button" onClick={() => set({ due: null })}>
              消す
            </button>
          )}
        </span>
      </label>
      <label className="tasks-form__field tasks-form__field--grow">
        <span>詳細</span>
        <textarea
          className="tasks-input tasks-form__detail"
          value={values.detail}
          onChange={(e) => set({ detail: e.target.value })}
        />
      </label>
      <div className="tasks-form__actions">
        <button type="submit" className="tasks-button tasks-button--primary" disabled={busy || !values.summary.trim()}>
          保存
        </button>
        <button type="button" className="tasks-button" disabled={busy} onClick={onCancel}>
          やめる
        </button>
      </div>
    </form>
  );
}

interface ConfirmProps {
  task: Task;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

// 削除の確認。戻せないので、既定の場所を「やめる」にする。
export function DeleteConfirm({ task, busy, onConfirm, onCancel }: ConfirmProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="pending-modal__backdrop">
      <div className="pending-modal tasks-confirm" role="alertdialog" aria-modal="true" aria-label="タスクの削除">
        <div className="pending-modal__header">
          <span className="pending-modal__title">タスクを削除します</span>
        </div>
        <div className="tasks-confirm__body">
          <p className="tasks-confirm__summary">{task.summary}</p>
          <p className="tasks-dim">ファイルを消します。削除すると元に戻せません。残しておきたいなら、完了にするとアーカイブに残ります。</p>
        </div>
        <div className="tasks-form__actions tasks-confirm__actions">
          <button type="button" className="tasks-button tasks-button--danger" disabled={busy} onClick={onConfirm}>
            削除する
          </button>
          <button type="button" className="tasks-button" autoFocus disabled={busy} onClick={onCancel}>
            やめる
          </button>
        </div>
      </div>
    </div>
  );
}

interface DetailProps {
  task: Task;
  busy: boolean;
  onEdit: () => void;
  onComplete: () => void;
  onRestore: () => void;
  onDelete: () => void;
}

// 選んだタスクの詳細と操作。詳細はAIも書くので、無害化してから出す(renderMarkdown)。
export function TaskDetail({ task, busy, onEdit, onComplete, onRestore, onDelete }: DetailProps) {
  const meta = [
    task.folder || "未分類",
    task.created && `作成 ${task.created}`,
    task.due && `締切 ${task.due}`,
    task.done && `完了 ${task.done}`,
  ]
    .filter(Boolean)
    .join(" ・ ");

  return (
    <div className="tasks-detail">
      <header className="tasks-detail__header">
        <h2 className="tasks-detail__title">{task.summary}</h2>
        <p className="tasks-detail__meta">{meta}</p>
        <div className="tasks-form__actions">
          {task.archived ? (
            <button type="button" className="tasks-button tasks-button--primary" disabled={busy} onClick={onRestore}>
              一覧へ戻す
            </button>
          ) : (
            <>
              <button type="button" className="tasks-button tasks-button--primary" disabled={busy} onClick={onComplete}>
                完了
              </button>
              <button type="button" className="tasks-button" disabled={busy || task.broken} onClick={onEdit}>
                編集
              </button>
            </>
          )}
          <button type="button" className="tasks-button" disabled={busy} onClick={onDelete}>
            削除
          </button>
        </div>
      </header>
      <div className="tasks-detail__scroll">
        {task.broken ? (
          <>
            <p className="tasks-error">形が崩れています。画面からは編集できないので、ファイル({task.id}.md)を直してください。</p>
            <pre className="tasks-detail__raw">{task.detail}</pre>
          </>
        ) : task.detail ? (
          // renderMarkdownで無害化済み。
          // eslint-disable-next-line react/no-danger
          <div
            className="article-detail__body"
            onClick={handleArticleLinkClick}
            dangerouslySetInnerHTML={{ __html: renderMarkdown(task.detail) }}
          />
        ) : (
          <p className="tasks-dim">詳細はありません。</p>
        )}
      </div>
    </div>
  );
}
