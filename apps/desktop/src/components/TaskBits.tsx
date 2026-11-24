import Icon from "../Icon";

/** Small, shared pieces for anything that draws a task or a run. */

export const PRIORITIES = ["urgent", "high", "medium", "low"] as const;
export type Priority = (typeof PRIORITIES)[number];

export function priorityLabel(priority: string): string {
  const value = priority === "med" ? "medium" : priority;
  return value ? value[0].toUpperCase() + value.slice(1) : "Medium";
}

/** Urgent is a red warning triangle; the rest are three bars, lit by rank. */
export function PriorityIcon({ priority }: { priority: string }) {
  const value = priority === "med" ? "medium" : priority;
  const label = `Priority: ${priorityLabel(value)}`;
  if (value === "urgent") {
    return (
      <span className="prio urgent" title={label} role="img" aria-label={label}>
        <Icon name="alert" size={13} />
      </span>
    );
  }
  const lit = value === "high" ? 3 : value === "low" ? 1 : 2;
  return (
    <span className="prio bars" title={label} role="img" aria-label={label}>
      {[1, 2, 3].map((bar) => (
        <i key={bar} className={bar <= lit ? "on" : undefined} />
      ))}
    </span>
  );
}

/** An initial in a circle. Agents and Lumi get the accent tint, people neutral. */
export function Avatar({ name, kind, size = "md" }: { name: string; kind?: "agent" | "lumi" | "human" | "server"; size?: "sm" | "md" }) {
  const initial = (name.trim()[0] ?? "?").toUpperCase();
  return (
    <span className={`av ${kind ?? "human"} ${size}`} aria-hidden="true">
      {initial}
    </span>
  );
}

const RUN_STATUS: Record<string, { label: string; tone: string }> = {
  queued: { label: "Queued", tone: "" },
  running: { label: "Working now", tone: "info" },
  succeeded: { label: "Completed", tone: "ok" },
  failed: { label: "Failed", tone: "bad" },
  cancelled: { label: "Cancelled", tone: "" },
  timed_out: { label: "Timed out", tone: "warn" },
};

export function RunStatusChip({ status }: { status: string }) {
  const entry = RUN_STATUS[status] ?? { label: status.replace(/_/g, " "), tone: "" };
  return (
    <span className={`chip ${entry.tone}`.trim()}>
      {status === "running" ? <span className="pulse" aria-hidden="true" /> : null}
      {entry.label}
    </span>
  );
}

export function isLiveRun(status: string | null | undefined): boolean {
  return status === "running" || status === "queued";
}

export function fmtTokens(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
  return String(value);
}

export function fmtUsd(value: number): string {
  return `$${value < 10 ? value.toFixed(2) : value.toFixed(0)}`;
}

export function fmtDuration(from: string | null | undefined, to: string | null | undefined): string | null {
  if (!from) return null;
  const ms = (to ? Date.parse(to) : Date.now()) - Date.parse(from);
  if (!Number.isFinite(ms) || ms < 0) return null;
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return `${Math.max(1, Math.round(ms / 1000))} s`;
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** `Nov 16 · 14:10`, or `14:10` today. */
export function fmtWhen(iso: string | null | undefined): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const time = date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  if (date.toDateString() === new Date().toDateString()) return time;
  return `${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })} · ${time}`;
}

/** `Nov 16` for a `YYYY-MM-DD` due date, read as a calendar day. */
export function fmtDue(day: string | null | undefined): string {
  if (!day) return "";
  const [year, month, date] = day.split("-").map(Number);
  if (!year || !month || !date) return day;
  return new Date(year, month - 1, date).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function isOverdue(day: string | null | undefined): boolean {
  if (!day) return false;
  const today = new Date();
  const local = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
  return day < local;
}

export function plural(count: number, word: string, many = `${word}s`): string {
  return `${count} ${count === 1 ? word : many}`;
}

/** Who still owes a turn in the task's status, in the workflow's order. */
export function TurnQueue({ issue }: { issue: { turnRemaining?: string[]; turnsUsed?: number } }) {
  if (!issue.turnRemaining || issue.turnRemaining.length === 0) return null;
  return (
    <span className="turn-queue" title="Turns left in this status">
      turn {(issue.turnsUsed ?? 0) + 1} · {issue.turnRemaining.join(", ")} still to go
    </span>
  );
}

/** A select's value for an assignee: `""`, `human:me` or `agent:<id>`. */
export function assigneeKey(assignee: { type: string; id: string } | null | undefined): string {
  return assignee ? `${assignee.type}:${assignee.id}` : "";
}

export function parseAssigneeKey(key: string): { assigneeType: "human" | "agent" | null; assigneeId: string | null } {
  if (!key) return { assigneeType: null, assigneeId: null };
  const [type, ...rest] = key.split(":");
  return { assigneeType: type === "agent" ? "agent" : "human", assigneeId: rest.join(":") };
}

/** Unassigned, You, then the agents. Agents assign themselves when they pick a task up. */
export function AssigneeOptions({ agents }: { agents: { id: string; name: string }[] }) {
  return (
    <>
      <option value="">Unassigned</option>
      <option value="human:me">You</option>
      {agents.map((agent) => (
        <option key={agent.id} value={`agent:${agent.id}`}>
          {agent.name} · agent
        </option>
      ))}
    </>
  );
}

export function PriorityOptions() {
  return (
    <>
      {PRIORITIES.map((value) => (
        <option key={value} value={value}>
          {priorityLabel(value)}
        </option>
      ))}
    </>
  );
}
