import type { DragEvent, MouseEvent } from "react";
import type { IssueSummary } from "../api";
import Icon from "../Icon";
import { useRunnerOnline } from "../runnerCapacity";
import { Avatar, PriorityIcon, fmtDue, isLiveRun, isOverdue, waitingFor } from "./TaskBits";

/** The MIME type a dragged card carries, so a column only accepts tasks. */
export const TASK_DRAG_TYPE = "application/x-hulahoop-task";

/** The meta line every task row shares: key, PR, files, live run, priority, due, assignee. */
export function TaskMeta({ issue, withKey = true }: { issue: IssueSummary; withKey?: boolean }) {
  const live = isLiveRun(issue.currentRun?.status);
  const waiting = waitingFor(issue.currentRun, useRunnerOnline());
  return (
    <span className="task-meta">
      {withKey ? <span className="task-key">{issue.key}</span> : null}
      {issue.status === "cancelled" ? <span className="task-cancelled">Cancelled</span> : null}
      {issue.prNumber ? <span className="task-pr">· PR #{issue.prNumber}</span> : null}
      {issue.fileCount ? (
        <span className="task-files" title={`${issue.fileCount} attached file${issue.fileCount === 1 ? "" : "s"}`}>
          <Icon name="clip" size={11} />
          {issue.fileCount}
        </span>
      ) : null}
      <span className="spacer" />
      {live ? (
        <span
          className="pulse running-dot"
          title={waiting ?? "An agent is working on this"}
          aria-label={waiting ? "Agent waiting for room" : "Agent working"}
        />
      ) : null}
      <PriorityIcon priority={issue.priority} />
      {issue.dueDate ? (
        <span className={isOverdue(issue.dueDate) ? "task-due overdue" : "task-due"} title={`Due ${issue.dueDate}`}>
          {fmtDue(issue.dueDate)}
        </span>
      ) : null}
      {issue.assignee ? (
        <span title={`Assigned to ${issue.assignee.name}`}>
          <Avatar name={issue.assignee.name} kind={issue.assignee.type === "agent" ? "agent" : "human"} size="sm" />
        </span>
      ) : null}
    </span>
  );
}

/** Why the task's run has not started yet, on its own line: the host has no
 *  room for it, or an older run goes first. Nothing once it runs. */
export function TaskWait({ issue }: { issue: IssueSummary }) {
  const waiting = waitingFor(issue.currentRun, useRunnerOnline());
  return waiting ? (
    <span className="task-wait truncate" title={waiting}>
      {waiting}
    </span>
  ) : null;
}

/** One task on the board: a button that opens it, drags to another column and
 *  answers a right click with the task's menu. */
export default function TaskCard({
  issue,
  selected,
  dragging,
  onOpen,
  onMenu,
  onDragStart,
  onDragEnd,
}: {
  issue: IssueSummary;
  selected: boolean;
  dragging: boolean;
  onOpen: (id: string) => void;
  onMenu: (issue: IssueSummary, event: MouseEvent<HTMLElement>) => void;
  onDragStart: (issue: IssueSummary) => void;
  onDragEnd: () => void;
}) {
  return (
    <button
      type="button"
      className={`task-card${dragging ? " dragging" : ""}`}
      aria-current={selected ? "true" : undefined}
      data-issue-id={issue.id}
      data-panel-opener=""
      draggable
      onClick={() => onOpen(issue.id)}
      onContextMenu={(event) => {
        event.preventDefault();
        onMenu(issue, event);
      }}
      onDragStart={(event: DragEvent<HTMLButtonElement>) => {
        event.dataTransfer.setData(TASK_DRAG_TYPE, issue.id);
        event.dataTransfer.effectAllowed = "move";
        onDragStart(issue);
      }}
      onDragEnd={onDragEnd}
    >
      <span className="card-title">{issue.title}</span>
      <TaskMeta issue={issue} />
      <TaskWait issue={issue} />
    </button>
  );
}
