import type { Task } from "../types";

// タスクの画面とホームの札が使う、絞り込み・並べ替え・締切の判定(詩織Ver6.2)。

export type TaskFilter =
  | { kind: "all" }
  | { kind: "uncategorized" }
  | { kind: "folder"; name: string }
  | { kind: "archive" };

export type TaskSort = "due" | "created" | "folder";

export type DueState = "overdue" | "near" | "none";

const NEAR_DAYS = 7;

function dateString(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

// 端末の日付で判定する。超過は今日より前、近いは今日から7日後ちょうどまで。
export function dueState(due: string | null, now: Date = new Date()): DueState {
  if (!due) return "none";
  const limit = new Date(now.getFullYear(), now.getMonth(), now.getDate() + NEAR_DAYS);
  if (due < dateString(now)) return "overdue";
  return due <= dateString(limit) ? "near" : "none";
}

// ホームの札に出すタスク(超過と7日以内)を、締切の近い順で返す。
export function dueSoonTasks(tasks: Task[], now: Date = new Date()): Task[] {
  return sortTasks(
    tasks.filter((t) => dueState(t.due, now) !== "none"),
    "due",
  );
}

// 親のフォルダを選ぶと、その中のフォルダのタスクも出す。
export function inFolder(task: Task, folder: string): boolean {
  return task.folder === folder || task.folder.startsWith(`${folder}/`);
}

export function matchesFilter(task: Task, filter: TaskFilter): boolean {
  switch (filter.kind) {
    case "all":
      return true;
    case "uncategorized":
      return task.folder === "";
    case "folder":
      return inFolder(task, filter.name);
    case "archive":
      return task.archived;
  }
}

function searchKey(text: string): string {
  return text.normalize("NFC").toLowerCase();
}

// 概要と詳細の部分一致。RAGを使わないので、RAGが止まっていても検索できる。
export function matchesQuery(task: Task, query: string): boolean {
  const key = searchKey(query.trim());
  return !key || searchKey(task.summary).includes(key) || searchKey(task.detail).includes(key);
}

function compareDue(a: Task, b: Task): number {
  if (a.due && b.due) return a.due.localeCompare(b.due);
  if (a.due) return -1;
  return b.due ? 1 : 0;
}

// 締切日は締切なしを最後に、作成日は新しい順に、フォルダは未分類を最後に並べる。
export function sortTasks(tasks: Task[], sort: TaskSort): Task[] {
  const byId = (a: Task, b: Task) => a.id.localeCompare(b.id);
  const compare = {
    due: (a: Task, b: Task) => compareDue(a, b) || byId(a, b),
    created: (a: Task, b: Task) => b.created.localeCompare(a.created) || byId(b, a),
    folder: (a: Task, b: Task) => {
      if (!a.folder !== !b.folder) return a.folder ? -1 : 1;
      return a.folder.localeCompare(b.folder) || compareDue(a, b) || byId(a, b);
    },
  }[sort];
  return [...tasks].sort(compare);
}
