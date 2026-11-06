import { dueState } from "../../lib/tasks";
import type { Task } from "../../types";

// 札に出す行数。多いときは「ほかN件」にまとめる。
const DUE_SLIP_MAX = 3;

interface Props {
  // 超過と7日以内のタスク(締切の近い順)。1件以上あるときだけ出す
  tasks: Task[];
  now: Date;
  onOpenList: () => void;
  onOpenTask: (id: string) => void;
}

// "2026-11-09"を、札の幅に入る"11/9"の形にする。
function shortDue(due: string): string {
  const [, month, day] = due.split("-");
  return `${Number(month)}/${Number(day)}`;
}

// ホームの締切の札(詩織Ver6.2)。見出しと「ほかN件」でタスクの画面へ、行でそのタスクへ移る。
function DueSlip({ tasks, now, onOpenList, onOpenTask }: Props) {
  const rest = tasks.length - DUE_SLIP_MAX;

  return (
    <section className="home-screen__memo home-screen__due" aria-label="締切が近いタスク">
      <button type="button" className="home-screen__memo-head home-screen__due-head" onClick={onOpenList}>
        締切の札
        <small>{tasks.length}件</small>
      </button>
      {tasks.slice(0, DUE_SLIP_MAX).map((task) => {
        const overdue = dueState(task.due, now) === "overdue";
        return (
          <button
            key={task.id}
            type="button"
            className="home-screen__due-line"
            title={task.summary}
            onClick={() => onOpenTask(task.id)}
          >
            <em className={overdue ? "home-screen__due-date--overdue" : "home-screen__due-date--near"}>
              {shortDue(task.due as string)}
              {overdue && " 超過"}
            </em>
            <span>{task.summary}</span>
          </button>
        );
      })}
      {rest > 0 && (
        <button type="button" className="home-screen__due-more" onClick={onOpenList}>
          ほか{rest}件
        </button>
      )}
    </section>
  );
}

export default DueSlip;
