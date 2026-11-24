import type { MouseEvent } from "react";
import type { IssueSummary } from "../api";
import type { Column } from "./TaskBoard";
import { Avatar, PriorityIcon, fmtDue, isLiveRun, isOverdue, priorityLabel } from "./TaskBits";

/**
 * Every task in one table, sorted by the workflow's column order. Each row is a
 * button that opens the task, so the rows are list items rather than table rows
 * (a `row` role would take the button's semantics away); the header is visual.
 */
export default function TaskList({
  issues,
  columns,
  selectedId,
  onOpen,
  onMenu,
}: {
  issues: IssueSummary[];
  columns: Column[];
  selectedId: string | null;
  onOpen: (id: string) => void;
  onMenu: (issue: IssueSummary, event: MouseEvent<HTMLElement>) => void;
}) {
  const order = new Map(columns.map((column, index) => [column.status as string, index]));
  const title = new Map(columns.map((column) => [column.status as string, column.title]));
  const rows = [...issues].sort(
    (a, b) => (order.get(a.status) ?? 99) - (order.get(b.status) ?? 99) || b.number - a.number,
  );

  if (rows.length === 0) {
    return (
      <div className="empty">
        <strong>No tasks here</strong>
        <span>Create one with New task, or switch to All to see finished work.</span>
      </div>
    );
  }

  return (
    <div className="task-list">
      <div className="task-list-head" aria-hidden="true">
        <span>ID</span>
        <span>Title</span>
        <span>Status</span>
        <span>Priority</span>
        <span>Assignee</span>
        <span>Due</span>
      </div>
      <div role="list" aria-label="Tasks">
        {rows.map((issue) => (
          <div role="listitem" key={issue.id}>
            <button
              type="button"
              className="task-row"
              aria-current={issue.id === selectedId ? "true" : undefined}
              data-issue-id={issue.id}
              data-panel-opener=""
              onClick={() => onOpen(issue.id)}
              onContextMenu={(event) => {
                event.preventDefault();
                onMenu(issue, event);
              }}
            >
              <span className="task-key">{issue.key}</span>
              <span className="task-row-title">
                <span className="truncate">{issue.title}</span>
                {isLiveRun(issue.currentRun?.status) ? <span className="pulse running-dot" aria-label="Agent working" /> : null}
              </span>
              <span className="task-row-status">
                <i className={`status-dot ${issue.status}`} aria-hidden="true" />
                {title.get(issue.status) ?? issue.status.replace(/_/g, " ")}
              </span>
              <span className="task-row-prio">
                <PriorityIcon priority={issue.priority} />
                {priorityLabel(issue.priority)}
              </span>
              <span className="task-row-who">
                {issue.assignee ? (
                  <>
                    <Avatar name={issue.assignee.name} kind={issue.assignee.type === "agent" ? "agent" : "human"} size="sm" />
                    <span className="truncate">{issue.assignee.name}</span>
                  </>
                ) : (
                  <span className="dim">—</span>
                )}
              </span>
              <span className={isOverdue(issue.dueDate) ? "task-due overdue" : "task-due"}>
                {issue.dueDate ? fmtDue(issue.dueDate) : "—"}
              </span>
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
