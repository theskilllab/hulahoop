import { useState } from "react";
import Icon from "../Icon";
import { gql, type Artifact, type IssueSummary as Issue, type Milestone, type Project } from "../api";
import PageHeader from "../PageHeader";
import { FALLBACK_COLUMNS, isDoneStatus, type Column } from "../components/TaskBoard";
import LinkTasksDialog from "../components/LinkTasksDialog";
import NewMilestoneDialog from "../components/NewMilestoneDialog";
import { StatusIcon, plural } from "../components/TaskBits";

interface Props {
  workspaceId: string;
  projects: Project[];
  milestones: Milestone[];
  issues: Issue[];
  artifacts: Artifact[];
  columns?: Column[];
  onChanged: () => Promise<void> | void;
  onOpenIssue: (id: string) => void;
  onOpenArtifacts: () => void;
}

const DAY = 864e5;

function parseDate(iso: string | null): Date | null {
  if (!iso) return null;
  const date = new Date(`${iso.slice(0, 10)}T00:00:00`);
  return Number.isNaN(date.getTime()) ? null : date;
}

function formatDate(iso: string | null): string {
  const date = parseDate(iso);
  return date ? date.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" }) : "—";
}

/** The workflow's column order, as the prototype orders a milestone's tasks; cancelled goes last. */
function statusOrder(columns: Column[]): string[] {
  const order: string[] = columns.map((column) => column.status);
  return order.includes("cancelled") ? order : [...order, "cancelled"];
}

function statusRank(order: string[], status: string): number {
  const rank = order.indexOf(status);
  return rank === -1 ? order.length : rank;
}

function statusLabel(columns: Column[], status: string): string {
  const column = columns.find((entry) => entry.status === status);
  if (column) return column.title;
  const text = status.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

interface Standing {
  tone: "ok" | "warn" | "bad" | "";
  label: string;
  left: string;
}

/** The chip and the countdown, from the milestone's own status and date and its issues' progress. */
function standing(milestone: Milestone, pct: number): Standing {
  if (milestone.status !== "open") return { tone: "", label: "Closed", left: "Closed" };
  const due = parseDate(milestone.targetDate);
  if (!due) return { tone: "ok", label: "On track", left: "No due date" };
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = Math.round((due.getTime() - today.getTime()) / DAY);
  if (days < 0) return { tone: "bad", label: "Overdue", left: `${plural(-days, "day")} overdue` };
  const left = days === 0 ? "Due today" : `${plural(days, "day")} left`;
  if (days <= 10 && pct < 50) return { tone: "warn", label: "At risk", left };
  return { tone: "ok", label: "On track", left };
}

/** Finished work counts toward progress, cancelled tasks included, as on the board. */
function progressOf(members: Issue[]) {
  const done = members.filter((issue) => isDoneStatus(issue.status)).length;
  return { done, pct: members.length ? Math.round((done / members.length) * 100) : 0 };
}

/**
 * Milestones are bodies of work with a name, a date and a shelf. Nothing enforces
 * that an issue contributes to one — the milestone is a label for humans, and the
 * loop keeps its own state machine on the issue.
 */
export default function Milestones({ workspaceId, projects, milestones, issues, artifacts, columns = FALLBACK_COLUMNS, onChanged, onOpenIssue, onOpenArtifacts }: Props) {
  const order = statusOrder(columns);
  const [busy, setBusy] = useState(false);
  const [linking, setLinking] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  /** Run a mutation from the detail pane; a failure is shown there instead of vanishing. */
  async function act(change: () => Promise<unknown>, refreshOnError = false) {
    if (busy) return;
    setBusy(true);
    setActionError(null);
    try {
      await change();
      await onChanged();
    } catch (cause) {
      setActionError(String(cause instanceof Error ? cause.message : cause));
      if (refreshOnError) await onChanged();
    } finally {
      setBusy(false);
    }
  }

  const close = (id: string, status: string) =>
    act(() =>
      gql(`mutation ($id: UUID!, $status: String!) { updateMilestone(id: $id, status: $status) { id status } }`, {
        id,
        status,
      }),
    );

  const setMilestone = (issueId: string, milestoneId: string | null) =>
    gql(
      `mutation ($issueId: UUID!, $milestoneId: UUID) { setIssueMilestone(issueId: $issueId, milestoneId: $milestoneId) { id milestoneId } }`,
      { issueId, milestoneId },
    );

  const unlink = (issueId: string) => act(() => setMilestone(issueId, null));

  /** Close the dialog first so a failure is readable in the pane, and refresh even when some tasks moved. */
  const link = (issueIds: string[], milestoneId: string) =>
    act(async () => {
      let linked = 0;
      try {
        for (const issueId of issueIds) {
          await setMilestone(issueId, milestoneId);
          linked += 1;
        }
      } catch (cause) {
        setLinking(null);
        const reason = cause instanceof Error ? cause.message : String(cause);
        throw new Error(`Linked ${linked} of ${issueIds.length} tasks: ${reason}`);
      }
      setLinking(null);
    }, true);

  const ordered = [...milestones].sort(
    (a, b) =>
      Number(a.status !== "open") - Number(b.status !== "open") ||
      (a.targetDate ?? "9").localeCompare(b.targetDate ?? "9"),
  );
  const current = ordered.find((entry) => entry.id === selected) ?? ordered[0] ?? null;
  const members = current
    ? issues.filter((issue) => issue.milestoneId === current.id).sort((a, b) => statusRank(order, a.status) - statusRank(order, b.status))
    : [];
  // A task can be moved here from another milestone, but only from the same project.
  const candidates = current
    ? issues.filter((issue) => issue.milestoneId !== current.id && (!current.projectId || issue.projectId === current.projectId))
    : [];
  // Files on the milestone itself, plus those attached to its tasks.
  const memberIds = new Set(members.map((issue) => issue.id));
  const files = current
    ? artifacts
        .filter((file) => (file.issueId === null ? file.milestoneId === current.id : memberIds.has(file.issueId)))
        .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
    : [];

  return (
    <section className="screen page">
      <PageHeader
        title="Milestones"
        subtitle={`${milestones.filter((entry) => entry.status === "open").length} open`}
        actions={
          <button type="button" className="primary" onClick={() => setCreating(true)}>
            <Icon name="plus" size={12} />
            New milestone
          </button>
        }
      />

      {creating ? (
        <NewMilestoneDialog
          workspaceId={workspaceId}
          projects={projects}
          onCreated={async (created) => {
            setSelected(created.id);
            await onChanged();
          }}
          onClose={() => setCreating(false)}
        />
      ) : null}

      <div className="msplit">
        <div className="mslist">
          {ordered.length === 0 && (
            <div className="empty">
              <strong>No milestones yet</strong>Create one to group tasks toward a goal.
            </div>
          )}
          {ordered.map((entry) => {
            const members = issues.filter((issue) => issue.milestoneId === entry.id);
            const { done, pct } = progressOf(members);
            const state = standing(entry, pct);
            return (
              <button
                key={entry.id}
                type="button"
                className="msrow"
                aria-current={entry.id === current?.id}
                onClick={() => {
                  setSelected(entry.id);
                  setActionError(null);
                }}
              >
                <span className="od-row">
                  <strong className="od-fill ms-name">{entry.title}</strong>
                  <span className={`chip ${state.tone}`}>
                    <Icon name={state.tone === "ok" || !state.tone ? "check" : "alert"} size={12} />
                    {state.label}
                  </span>
                </span>
                <span className="od-row ms-meta">
                  {entry.targetDate && <span className="ms-nowrap">{formatDate(entry.targetDate)}</span>}
                  <span className="od-fill ms-name">{entry.targetDate ? `· ${state.left}` : state.left}</span>
                  <span className="ms-nowrap">
                    {done}/{members.length} done
                  </span>
                </span>
                <span
                  className="prog"
                  role="progressbar"
                  aria-label={`${entry.title} progress`}
                  aria-valuenow={pct}
                  aria-valuemin={0}
                  aria-valuemax={100}
                >
                  <i style={{ width: `${pct}%` }} />
                </span>
              </button>
            );
          })}
        </div>

        <div className="msdetail">
          {current ? (
            <Detail
              milestone={current}
              columns={columns}
              order={order}
              members={members}
              candidates={candidates}
              files={files}
              milestoneTitles={new Map(milestones.map((entry) => [entry.id, entry.title]))}
              project={projects.find((entry) => entry.id === current.projectId)}
              busy={busy}
              error={actionError}
              linking={linking === current.id}
              setLinking={(on) => setLinking(on ? current.id : null)}
              onToggle={() => void close(current.id, current.status === "open" ? "closed" : "open")}
              onLink={(issueIds) => void link(issueIds, current.id)}
              onUnlink={(issueId) => void unlink(issueId)}
              onOpenIssue={onOpenIssue}
              onOpenArtifacts={onOpenArtifacts}
            />
          ) : null}
        </div>
      </div>
    </section>
  );
}

interface DetailProps {
  milestone: Milestone;
  columns: Column[];
  order: string[];
  members: Issue[];
  candidates: Issue[];
  files: Artifact[];
  milestoneTitles: Map<string, string>;
  project: Project | undefined;
  busy: boolean;
  error: string | null;
  linking: boolean;
  setLinking: (on: boolean) => void;
  onToggle: () => void;
  onLink: (issueIds: string[]) => void;
  onUnlink: (issueId: string) => void;
  onOpenIssue: (id: string) => void;
  onOpenArtifacts: () => void;
}

function Detail({
  milestone,
  columns,
  order,
  members,
  candidates,
  files,
  milestoneTitles,
  project,
  busy,
  error,
  linking,
  setLinking,
  onToggle,
  onLink,
  onUnlink,
  onOpenIssue,
  onOpenArtifacts,
}: DetailProps) {
  const { done, pct } = progressOf(members);
  const state = standing(milestone, pct);
  const open = milestone.status === "open";
  const statuses = [...order, ...new Set(members.map((issue) => issue.status).filter((status) => !order.includes(status)))];
  const counts = statuses.map((status) => ({
    status,
    n: members.filter((issue) => issue.status === status).length,
  })).filter((entry) => entry.n > 0);
  const keyOf = new Map(members.map((issue) => [issue.id, issue.key]));
  const where = project ? `${project.prefix ?? "—"} · ${project.name}` : "Workspace";
  return (
    <div className="msd">
      <div className="od-row ms-title">
        <h2 className="od-fill">{milestone.title}</h2>
        <span className={`chip ${state.tone}`}>
          <Icon name={state.tone === "ok" || !state.tone ? "check" : "alert"} size={12} />
          {state.label}
        </span>
      </div>
      <p className="ms-sub">
        {milestone.targetDate ? `Due ${formatDate(milestone.targetDate)} · ` : ""}
        {state.left} · {where}
      </p>
      {milestone.body && <p className="ms-body">{milestone.body}</p>}

      <div className="od-stack tight ms-progress">
        <span className="od-row">
          <strong className="od-fill">{pct}% complete</strong>
          <span className="ms-sub">
            {done} of {members.length} done
          </span>
        </span>
        <span
          className="prog big"
          role="progressbar"
          aria-label="Progress"
          aria-valuenow={pct}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <i style={{ width: `${pct}%` }} />
        </span>
        {counts.length > 0 && (
          <span className="ms-counts">
            {counts.map((entry) => (
              <span key={entry.status} className="ms-count">
                <StatusIcon status={entry.status} />
                {statusLabel(columns, entry.status)} {entry.n}
              </span>
            ))}
          </span>
        )}
      </div>

      <div className="ms-actions">
        <button type="button" className="primary" onClick={() => setLinking(true)} disabled={!open || busy}>
          <Icon name="plus" size={12} />
          Link tasks
        </button>
        <button type="button" className="secondary" onClick={onToggle} disabled={busy}>
          {open ? "Close milestone" : "Reopen milestone"}
        </button>
      </div>
      {linking && (
        <LinkTasksDialog
          candidates={candidates}
          milestoneTitles={milestoneTitles}
          busy={busy}
          onLink={onLink}
          onClose={() => setLinking(false)}
        />
      )}
      {error && <p className="error">{error}</p>}

      <div className="field-label ms-heading">Artifacts · {files.length}</div>
      {files.length ? (
        <div className="mt-list">
          {files.map((file) => (
            <div key={file.id} className="mt-row">
              <button type="button" className="mt-main" onClick={onOpenArtifacts}>
                <Icon name="file" size={12} />
                <span className="od-fill ms-name">{file.title || file.filename}</span>
                {file.issueId && keyOf.has(file.issueId) && <span className="mono muted ms-nowrap">via {keyOf.get(file.issueId)}</span>}
                <span className="chip">{file.kind}</span>
                <span className="muted ms-nowrap">{formatDate(file.createdAt)}</span>
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div className="empty ms-empty">
          <strong>No artifacts yet</strong>Add a release plan, a spec or any file that belongs to this milestone as a whole. Artifacts on its tasks show up here too.
        </div>
      )}

      <div className="field-label ms-heading">Tasks · {members.length}</div>
      {members.length ? (
        <div className="mt-list">
          {members.map((issue) => (
            <div key={issue.id} className="mt-row">
              <button type="button" className="mt-main" onClick={() => onOpenIssue(issue.id)}>
                <span className="mono muted ms-nowrap">{issue.key}</span>
                <span className="od-fill ms-name">{issue.title}</span>
                <span className="ms-nowrap">
                  <StatusIcon status={issue.status} /> {statusLabel(columns, issue.status)}
                </span>
              </button>
              <button
                type="button"
                className="icon-btn"
                onClick={() => onUnlink(issue.id)}
                aria-label={`Unlink ${issue.key} from this milestone`}
                title="Unlink"
              >
                <Icon name="close" size={12} />
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div className="empty ms-empty">
          <strong>No tasks linked yet</strong>Use “Link tasks”, or pick this milestone from any task’s properties.
        </div>
      )}
    </div>
  );
}
