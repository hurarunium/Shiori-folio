import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../api/tauri";
import { dueState, matchesFilter, matchesQuery, sortTasks, type TaskFilter, type TaskSort } from "../../lib/tasks";
import { useMainViewStore } from "../../store/useMainViewStore";
import type { Task } from "../../types";
import { DeleteConfirm, TaskDetail, TaskForm, type TaskFormValues } from "./TaskDetail";
import TaskFolderNav from "./TaskFolderNav";
import "./TasksScreen.css";

interface TasksData {
  active: Task[];
  archived: Task[];
  folders: string[];
}

// 右の列の状態。編集は、始めたときの1件を持つ(かち合いの検知に、そのときのmtimeを使う)。
type Pane = { mode: "view" } | { mode: "new" } | { mode: "edit"; task: Task };

const SORT_LABELS: Record<TaskSort, string> = { due: "締切日", created: "作成日", folder: "フォルダ" };

// タスクの画面(詩織Ver6.2)。左にフォルダ、中央に一覧、右に詳細の3列。
// AIも同じmdを書き換えるので、この画面が開かれるたびと、書き込みのあとに読み直す。
// データの読み書きと選択の状態を持ち、列の表示は子の部品に任せる。
function TasksScreen() {
  const isActive = useMainViewStore((s) => s.active === "tasks");
  const [data, setData] = useState<TasksData | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [filter, setFilter] = useState<TaskFilter>({ kind: "all" });
  const [sort, setSort] = useState<TaskSort>("due");
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [pane, setPane] = useState<Pane>({ mode: "view" });
  const [confirming, setConfirming] = useState<Task | null>(null);
  const [busy, setBusy] = useState(false);
  const loadSerial = useRef(0);

  const reload = useCallback(async () => {
    const serial = ++loadSerial.current;
    try {
      const [active, archived, folders] = await Promise.all([
        api.listTasks(false),
        api.listTasks(true),
        api.listTaskFolders(),
      ]);
      if (serial !== loadSerial.current) return;
      setData({ active, archived, folders });
      setLoadError(null);
    } catch (err) {
      if (serial === loadSerial.current) setLoadError(String(err));
    }
  }, []);

  useEffect(() => {
    if (isActive) reload();
  }, [isActive, reload]);

  // ホームの札から渡されたタスクを、隠れないように絞り込みを外して選ぶ。編集中の欄は閉じない。
  const taskToOpen = useMainViewStore((s) => s.taskToOpen);
  const clearTaskToOpen = useMainViewStore((s) => s.clearTaskToOpen);
  useEffect(() => {
    if (!taskToOpen) return;
    setFilter({ kind: "all" });
    setQuery("");
    setSelectedId(taskToOpen);
    clearTaskToOpen();
  }, [taskToOpen, clearTaskToOpen]);

  // 書き込みのあとは、失敗しても読み直す(ほかで変わっていることがあるため)。成功した結果を返す。
  const write = async <T,>(action: () => Promise<T>): Promise<T | null> => {
    setBusy(true);
    setActionError(null);
    try {
      return await action();
    } catch (err) {
      setActionError(String(err));
      return null;
    } finally {
      await reload();
      setBusy(false);
    }
  };

  const inArchive = filter.kind === "archive";
  const shown = useMemo(() => {
    const source = inArchive ? data?.archived : data?.active;
    return sortTasks((source ?? []).filter((t) => matchesFilter(t, filter) && matchesQuery(t, query)), sort);
  }, [data, filter, inArchive, query, sort]);
  const selected = shown.find((t) => t.id === selectedId) ?? shown[0] ?? null;

  // 保存したタスクが、いまの絞り込みで隠れないようにする。
  const reveal = (task: Task) => {
    setSelectedId(task.id);
    if (!matchesFilter(task, filter)) {
      setFilter(task.folder ? { kind: "folder", name: task.folder } : { kind: "uncategorized" });
    }
    if (!matchesQuery(task, query)) setQuery("");
  };

  const saveForm = async (values: TaskFormValues) => {
    const task = await write(() => {
      if (pane.mode === "edit") {
        return api.saveTask({ ...values, id: pane.task.id, expectedMtime: pane.task.mtime });
      }
      return api.addTask(values);
    });
    if (task) {
      reveal(task);
      setPane({ mode: "view" });
    }
  };

  const deleteConfirmed = async () => {
    if (!confirming) return;
    await write(() => api.deleteTask(confirming.id));
    setConfirming(null);
  };

  const addFolder = async (name: string) => {
    await api.addTaskFolder(name);
    await reload();
  };

  // 選んでいるフォルダの名前が変わったら、選択もついて行く。
  const renameFolder = async (from: string, to: string) => {
    const renamed = await api.renameTaskFolder(from, to);
    if (filter.kind === "folder" && (filter.name === from || filter.name.startsWith(`${from}/`))) {
      setFilter({ kind: "folder", name: renamed + filter.name.slice(from.length) });
    }
    await reload();
  };

  const removeFolder = async (name: string) => {
    await api.removeTaskFolder(name);
    if (filter.kind === "folder" && filter.name === name) setFilter({ kind: "all" });
    await reload();
  };

  const editing = pane.mode !== "view";
  const defaultFolder = filter.kind === "folder" ? filter.name : "";

  const emptyMessage = () => {
    if (query.trim()) return "該当するタスクがありません";
    if (inArchive) return "アーカイブは空です";
    if (data?.active.length === 0) return "タスクはまだありません";
    return "ここにタスクはありません";
  };

  return (
    <div className="tasks-screen">
      <TaskFolderNav
        tasks={data?.active ?? []}
        archivedCount={data?.archived.length ?? 0}
        folders={data?.folders ?? []}
        filter={filter}
        onFilter={setFilter}
        onAddFolder={addFolder}
        onRenameFolder={renameFolder}
        onRemoveFolder={removeFolder}
      />

      <section className="tasks-list" aria-label="タスクの一覧">
        <div className="tasks-list__tools">
          <input
            type="search"
            className="tasks-input tasks-list__search"
            placeholder="概要と詳細を検索"
            aria-label="タスクを検索"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <select
            className="tasks-input"
            aria-label="並べ替え"
            value={sort}
            onChange={(e) => setSort(e.target.value as TaskSort)}
          >
            {(Object.keys(SORT_LABELS) as TaskSort[]).map((key) => (
              <option key={key} value={key}>
                {SORT_LABELS[key]}
              </option>
            ))}
          </select>
          <button
            type="button"
            className="tasks-button tasks-button--primary"
            disabled={editing || inArchive}
            onClick={() => {
              setActionError(null);
              setPane({ mode: "new" });
            }}
          >
            追加
          </button>
        </div>

        <div className="tasks-list__scroll">
          {loadError && <p className="tasks-error">{loadError}</p>}
          {!data && !loadError && <p className="tasks-dim">読み込んでいます…</p>}
          {data && shown.length === 0 && (
            <div className="tasks-list__empty">
              <p className="tasks-dim">{emptyMessage()}</p>
              {data.active.length === 0 && !inArchive && !editing && !query.trim() && (
                <button
                  type="button"
                  className="tasks-button tasks-button--primary"
                  onClick={() => setPane({ mode: "new" })}
                >
                  追加
                </button>
              )}
            </div>
          )}
          {shown.map((task) => {
            const state = task.archived ? "none" : dueState(task.due);
            return (
              <button
                key={task.id}
                type="button"
                disabled={editing}
                className={`tasks-row${task.id === selected?.id ? " tasks-row--active" : ""}`}
                onClick={() => {
                  setActionError(null);
                  setSelectedId(task.id);
                }}
              >
                <span className="tasks-row__main">
                  <span className="tasks-row__summary">
                    {task.broken && <span className="tasks-row__broken">形が崩れています</span>}
                    {task.summary}
                  </span>
                  <span className="tasks-row__meta">
                    <span>{task.folder || "未分類"}</span>
                    {task.done ? (
                      <span>完了 {task.done}</span>
                    ) : (
                      task.due && <span className={`tasks-row__due tasks-row__due--${state}`}>締切 {task.due}</span>
                    )}
                    {task.created && <span className="tasks-row__created-inline">作成 {task.created}</span>}
                  </span>
                </span>
                <span className="tasks-row__created">{task.created}</span>
              </button>
            );
          })}
        </div>
      </section>

      <section className="tasks-pane" aria-label="タスクの詳細">
        {actionError && <p className="tasks-error">{actionError}</p>}
        {pane.mode === "new" && (
          <TaskForm
            title="タスクを追加"
            initial={{ summary: "", folder: defaultFolder, due: null, detail: "" }}
            folders={data?.folders ?? []}
            busy={busy}
            onSave={saveForm}
            onCancel={() => {
              setActionError(null);
              setPane({ mode: "view" });
            }}
          />
        )}
        {pane.mode === "edit" && (
          <TaskForm
            key={pane.task.id}
            title="タスクを編集"
            initial={{
              summary: pane.task.summary,
              folder: pane.task.folder,
              due: pane.task.due,
              detail: pane.task.detail,
            }}
            folders={data?.folders ?? []}
            busy={busy}
            onSave={saveForm}
            onCancel={() => {
              setActionError(null);
              setPane({ mode: "view" });
            }}
          />
        )}
        {pane.mode === "view" && selected && (
          <TaskDetail
            task={selected}
            busy={busy}
            onEdit={() => {
              setActionError(null);
              setPane({ mode: "edit", task: selected });
            }}
            onComplete={() => write(() => api.completeTask(selected.id))}
            onRestore={() => write(() => api.restoreTask(selected.id))}
            onDelete={() => setConfirming(selected)}
          />
        )}
        {pane.mode === "view" && !selected && data && <p className="tasks-dim">タスクを選ぶと、ここに詳細が出ます。</p>}
      </section>

      {confirming && (
        <DeleteConfirm task={confirming} busy={busy} onConfirm={deleteConfirmed} onCancel={() => setConfirming(null)} />
      )}
    </div>
  );
}

export default TasksScreen;
