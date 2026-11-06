import { useEffect, useState, type FormEvent } from "react";
import { inFolder, type TaskFilter } from "../../lib/tasks";
import type { Task } from "../../types";

interface Props {
  tasks: Task[];
  archivedCount: number;
  folders: string[];
  filter: TaskFilter;
  onFilter: (filter: TaskFilter) => void;
  onAddFolder: (name: string) => Promise<void>;
  onRenameFolder: (from: string, to: string) => Promise<void>;
  onRemoveFolder: (name: string) => Promise<void>;
}

function isSame(a: TaskFilter, b: TaskFilter): boolean {
  return a.kind === b.kind && (a.kind !== "folder" || a.name === (b as { name: string }).name);
}

// タスクの画面の左の列。すべて・フォルダの木(2段)・未分類・アーカイブと、フォルダの管理。
// 表示とフォルダの入力だけを受け持ち、読み書きは親に任せる。
function TaskFolderNav({
  tasks,
  archivedCount,
  folders,
  filter,
  onFilter,
  onAddFolder,
  onRenameFolder,
  onRemoveFolder,
}: Props) {
  const [adding, setAdding] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<{ from: string; to: string } | null>(null);
  const [menu, setMenu] = useState<{ folder: string; x: number; y: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // 右クリックのメニューは、ほかを押すかEscで閉じる。
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  // 入力をやめたら、その入力のエラーも一緒に消す。
  const closeInputs = () => {
    setAdding(null);
    setRenaming(null);
    setError(null);
  };

  const attempt = async (action: () => Promise<void>, done: () => void) => {
    setError(null);
    try {
      await action();
      done();
    } catch (err) {
      setError(String(err));
    }
  };

  const submitAdd = (e: FormEvent) => {
    e.preventDefault();
    const name = adding?.trim();
    if (!name) {
      closeInputs();
      return;
    }
    attempt(
      () => onAddFolder(name),
      () => setAdding(null),
    );
  };

  const submitRename = (e: FormEvent) => {
    e.preventDefault();
    if (!renaming) return;
    const to = renaming.to.trim();
    if (!to || to === renaming.from) {
      closeInputs();
      return;
    }
    attempt(
      () => onRenameFolder(renaming.from, to),
      () => setRenaming(null),
    );
  };

  const row = (label: string, count: number, target: TaskFilter, modifier = "") => (
    <button
      type="button"
      className={`tasks-nav__item${modifier}${isSame(filter, target) ? " tasks-nav__item--active" : ""}`}
      onClick={() => {
        setError(null);
        onFilter(target);
      }}
    >
      <span className="tasks-nav__label">{label}</span>
      <span className="tasks-nav__count">{count}</span>
    </button>
  );

  return (
    <nav className="tasks-nav" aria-label="タスクのフォルダ">
      {row("すべて", tasks.length, { kind: "all" })}
      {folders.map((folder) => {
        const nested = folder.includes("/");
        if (renaming?.from === folder) {
          return (
            <form key={folder} className="tasks-nav__form" onSubmit={submitRename}>
              <input
                autoFocus
                className="tasks-input"
                aria-label={`${folder}の新しい名前`}
                value={renaming.to}
                onChange={(e) => setRenaming({ from: folder, to: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === "Escape") closeInputs();
                }}
                onBlur={closeInputs}
              />
            </form>
          );
        }
        return (
          <div
            key={folder}
            onContextMenu={(e) => {
              e.preventDefault();
              setMenu({ folder, x: e.clientX, y: e.clientY });
            }}
          >
            {row(
              nested ? folder.slice(folder.indexOf("/") + 1) : folder,
              tasks.filter((t) => inFolder(t, folder)).length,
              { kind: "folder", name: folder },
              nested ? " tasks-nav__item--nested" : "",
            )}
          </div>
        );
      })}
      {row("未分類", tasks.filter((t) => t.folder === "").length, { kind: "uncategorized" })}

      {adding === null ? (
        <button type="button" className="tasks-nav__add" onClick={() => setAdding("")}>
          ＋ フォルダを追加
        </button>
      ) : (
        <form className="tasks-nav__form" onSubmit={submitAdd}>
          <input
            autoFocus
            className="tasks-input"
            placeholder="例: Unity/Tanker"
            aria-label="新しいフォルダの名前"
            value={adding}
            onChange={(e) => setAdding(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") closeInputs();
            }}
          />
        </form>
      )}
      {error && <p className="tasks-error">{error}</p>}

      <div className="tasks-nav__bottom">{row("アーカイブ", archivedCount, { kind: "archive" })}</div>

      {menu && (
        <div
          className="tasks-nav__menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setError(null);
              setRenaming({ from: menu.folder, to: menu.folder });
              setMenu(null);
            }}
          >
            名前を変える
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              const folder = menu.folder;
              setMenu(null);
              attempt(
                () => onRemoveFolder(folder),
                () => undefined,
              );
            }}
          >
            削除
          </button>
        </div>
      )}
    </nav>
  );
}

export default TaskFolderNav;
