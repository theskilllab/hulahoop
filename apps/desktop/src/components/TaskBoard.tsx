import { useState, type MouseEvent } from "react";
import type { IssueStatus, IssueSummary } from "../api";
import TaskCard, { TASK_DRAG_TYPE } from "./TaskCard";

export type Column = { status: IssueStatus; title: string; owner: string };

/** The board is a view of the workspace's workflow document, so the columns and
 *  their wording arrive as props. This is the fallback before one has loaded. */
export const FALLBACK_COLUMNS: Column[] = [
  { status: "backlog", title: "Backlog", owner: "human" },
  { status: "todo", title: "To do", owner: "human" },
  { status: "in_progress", title: "In progress", owner: "agent" },
  { status: "blocked", title: "Blocked", owner: "human" },
  { status: "in_review", title: "In review", owner: "human" },
  { status: "done", title: "Done", owner: "human" },
];

export function isDoneStatus(status: string): boolean {
  return status === "done" || status === "cancelled";
}

/**
 * Columns from the workflow, cards inside. A dragged card only lights up and
 * drops on the columns its status declares as moves (the server checks them
 * again). Cancelled tasks sit in the Done column, the way the Active/All count
 * already counts them.
 */
export default function TaskBoard({
  issues,
  columns,
  selectedId,
  hiddenDone,
  onOpen,
  onMenu,
  onMove,
  onShowDone,
}: {
  issues: IssueSummary[];
  columns: Column[];
  selectedId: string | null;
  /** Done tasks the Active view left on the server. */
  hiddenDone: number;
  onOpen: (id: string) => void;
  onMenu: (issue: IssueSummary, event: MouseEvent<HTMLElement>) => void;
  onMove: (issue: IssueSummary, status: IssueStatus) => void;
  onShowDone: () => void;
}) {
  const [dragging, setDragging] = useState<IssueSummary | null>(null);
  const [over, setOver] = useState<IssueStatus | null>(null);
  const doneColumn = columns.find((column) => column.status === "done")?.status ?? null;
  const columnOf = (status: IssueStatus): IssueStatus =>
    doneColumn && isDoneStatus(status) && !columns.some((column) => column.status === status) ? doneColumn : status;
  const accepts = (issue: IssueSummary | null, status: IssueStatus) =>
    Boolean(issue && issue.status !== status && issue.moves?.includes(status));

  return (
    <div className="task-board" role="list" aria-label="Board">
      {columns.map((column) => {
        const items = issues.filter((issue) => columnOf(issue.status) === column.status);
        const legal = accepts(dragging, column.status);
        const foldedDone = isDoneStatus(column.status) && hiddenDone > 0;
        return (
          <section
            key={column.status}
            role="listitem"
            aria-label={`${column.title}, ${items.length}`}
            className={`task-col ${column.status}${over === column.status ? " over" : ""}${dragging && !legal && columnOf(dragging.status) !== column.status ? " closed" : ""}`}
            onDragOver={(event) => {
              if (!legal) return;
              if (!event.dataTransfer.types.includes(TASK_DRAG_TYPE)) return;
              event.preventDefault();
              event.dataTransfer.dropEffect = "move";
              if (over !== column.status) setOver(column.status);
            }}
            onDragLeave={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(null);
            }}
            onDrop={(event) => {
              setOver(null);
              const id = event.dataTransfer.getData(TASK_DRAG_TYPE);
              const issue = issues.find((entry) => entry.id === id) ?? null;
              if (!issue || !accepts(issue, column.status)) return;
              event.preventDefault();
              onMove(issue, column.status);
            }}
          >
            <h2 className="task-col-head" title={column.owner === "agent" ? "An agent works this column" : "A person works this column"}>
              <i className={`status-dot ${column.status}`} aria-hidden="true" />
              {column.title}
              <span className="n">{items.length}</span>
            </h2>
            <div className="task-col-body">
              {items.map((issue) => (
                <TaskCard
                  key={issue.id}
                  issue={issue}
                  selected={issue.id === selectedId}
                  dragging={dragging?.id === issue.id}
                  onOpen={onOpen}
                  onMenu={onMenu}
                  onDragStart={setDragging}
                  onDragEnd={() => {
                    setDragging(null);
                    setOver(null);
                  }}
                />
              ))}
              {foldedDone ? (
                <p className="task-col-empty">
                  {hiddenDone} done task{hiddenDone === 1 ? "" : "s"} not loaded.{" "}
                  <button type="button" className="link" onClick={onShowDone}>
                    Show them
                  </button>
                </p>
              ) : items.length === 0 ? (
                <p className="task-col-empty">Nothing here. Drag a task in.</p>
              ) : null}
            </div>
          </section>
        );
      })}
    </div>
  );
}
