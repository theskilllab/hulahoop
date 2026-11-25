import { useCallback, useEffect, useRef, useState } from "react";
import {
  gql,
  IssueGoneError,
  loadIssueDetail,
  moveIssue,
  updateIssue,
  type Agent,
  type Issue,
  type Milestone,
  type Project,
  type Run,
} from "../api";
import Activity from "../components/Activity";
import FilesSection from "../components/FilesSection";
import MarkdownView from "../components/MarkdownView";
import { normalizeMarkdown } from "../components/markdown";
import { type Column } from "../components/TaskBoard";
import {
  AssigneeOptions,
  PriorityOptions,
  RunStatusChip,
  TurnQueue,
  assigneeKey,
  isLiveRun,
  parseAssigneeKey,
  waitingFor,
} from "../components/TaskBits";
import RichText from "../RichText";
import { useRunnerOnline } from "../runnerCapacity";

const POLL_MS = 2500;

/**
 * One task with everything the detail view draws, kept fresh while it is open.
 * The board only carries summaries; this is the one place the full task loads.
 */
export function useIssueDetail(id: string | null) {
  const [issue, setIssue] = useState<Issue | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(id);
  current.current = id;
  // Only the newest request may land: a poll that started before a save must
  // not paint the old values back over the reload that follows the save.
  const latest = useRef(0);
  const inFlight = useRef(false);

  const reload = useCallback(async () => {
    const wanted = current.current;
    if (!wanted) return;
    const ticket = ++latest.current;
    inFlight.current = true;
    try {
      const next = await loadIssueDetail(wanted);
      if (ticket !== latest.current || current.current !== wanted) return;
      setIssue(next);
      setError(null);
    } catch (cause) {
      if (ticket !== latest.current || current.current !== wanted) return;
      // A task deleted while open must not keep looking editable: every action
      // on it would fail. Other errors (a dropped connection) keep the last copy.
      if (cause instanceof IssueGoneError) setIssue(null);
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (ticket === latest.current) inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    setIssue((previous) => (previous && previous.id === id ? previous : null));
    setError(null);
    if (!id) return;
    void reload();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible" && !inFlight.current) void reload();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [id, reload]);

  return { issue, error, reload };
}

/** What a person has typed on a task and not sent yet. The task screen keeps
 *  these per task, so opening another task and coming back loses nothing. */
export type TaskDraft = {
  comment?: string;
  /** Set while the description is being edited; `undefined` means not editing. */
  description?: string;
};

export type DraftUpdate = (key: keyof TaskDraft, update: (current: string | undefined) => string | undefined) => void;

type Props = {
  issue: Issue;
  draft: TaskDraft;
  onDraft: DraftUpdate;
  workspaceId: string;
  agents: Agent[];
  projects: Project[];
  milestones: Milestone[];
  columns: Column[];
  onChanged: () => void | Promise<void>;
  onOpenTranscript: (run: Run) => void;
  onSaved: (message: string) => void;
  onError: (message: string) => void;
};

/** The task itself: what it asks, its files, its activity and its properties. */
export default function IssueDetail({
  issue,
  draft,
  onDraft,
  workspaceId,
  agents,
  projects,
  milestones,
  columns,
  onChanged,
  onOpenTranscript,
  onSaved,
  onError,
}: Props) {
  const [busy, setBusy] = useState<string | null>(null);

  /** Runs an action, refreshes, and says whether the action went through. */
  async function run(label: string, action: () => Promise<unknown>, saved?: string): Promise<boolean> {
    setBusy(label);
    try {
      await action();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
      setBusy(null);
      return false;
    }
    try {
      await onChanged();
      if (saved) onSaved(saved);
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
    return true;
  }

  const save = (patch: Parameters<typeof updateIssue>[1], saved = "Saved") =>
    run("save", () => updateIssue(issue.id, patch), saved);

  return (
    <div className="tpage">
      <div className="tpage-main">
        <TitleField title={issue.title} onSave={(title) => save({ title })} />
        <DescriptionField
          body={issue.body}
          draft={draft.description}
          onDraft={(update) => onDraft("description", update)}
          onSave={(body) => save({ body })}
        />
        <FilesSection
          files={issue.files ?? []}
          workspaceId={workspaceId}
          projectId={issue.projectId}
          issueId={issue.id}
          onChanged={onChanged}
          onError={onError}
        />
        <Activity
          issue={issue}
          agents={agents}
          draft={draft.comment ?? ""}
          onDraft={(update) => onDraft("comment", (current) => update(current ?? ""))}
          onOpenTranscript={onOpenTranscript}
          onChanged={onChanged}
          onError={onError}
        />
      </div>
      <aside className="tpage-side" aria-label="Task properties">
        <Properties
          issue={issue}
          agents={agents}
          projects={projects}
          milestones={milestones}
          columns={columns}
          busy={busy}
          run={run}
          save={save}
        />
        <Actions issue={issue} busy={busy} run={run} />
      </aside>
    </div>
  );
}

function TitleField({ title, onSave }: { title: string; onSave: (title: string) => void }) {
  const [draft, setDraft] = useState(title);
  const cancelled = useRef(false);
  useEffect(() => setDraft(title), [title]);
  const commit = () => {
    if (cancelled.current) {
      cancelled.current = false;
      setDraft(title);
      return;
    }
    const next = draft.replace(/\s+/g, " ").trim();
    if (!next) setDraft(title);
    else if (next !== title) onSave(next);
  };
  return (
    <h2 className="tpage-title">
      <textarea
        aria-label="Title"
        rows={1}
        value={draft}
        onChange={(event) => setDraft(event.target.value.replace(/\n/g, " "))}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.blur();
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            cancelled.current = true;
            event.currentTarget.blur();
          }
        }}
      />
    </h2>
  );
}

/** Raw HTML, which the formatted editor would turn into plain text. */
const RAW_HTML = /<\/?[A-Za-z][^>]*>/;

/**
 * The description, read as markdown and edited as formatted text or, for a body
 * the formatted editor cannot hold (raw HTML, say), as the markdown source. It
 * saves only when the text changed: opening the editor and pressing Done on an
 * agent's description writes nothing back, and a formatted edit that comes out
 * the same as the original is not a change either.
 */
function DescriptionField({
  body,
  draft,
  onDraft,
  onSave,
}: {
  body: string;
  /** The text being edited, or undefined when not editing. */
  draft: string | undefined;
  onDraft: (update: (current: string | undefined) => string | undefined) => void;
  onSave: (body: string) => Promise<boolean>;
}) {
  const editing = draft !== undefined;
  const [source, setSource] = useState(() => RAW_HTML.test(body));
  const [saving, setSaving] = useState(false);
  // A source edit is saved as typed; a formatted one is compared by meaning.
  const typedSource = useRef(false);

  function start() {
    typedSource.current = false;
    setSource(RAW_HTML.test(body));
    onDraft(() => body);
  }

  function changed(next: string): boolean {
    if (next.trim() === body.trim()) return false;
    if (typedSource.current) return true;
    return normalizeMarkdown(next) !== normalizeMarkdown(body);
  }

  async function done() {
    if (draft === undefined || saving) return;
    const next = draft;
    if (!changed(next)) {
      onDraft(() => undefined);
      return;
    }
    setSaving(true);
    const saved = await onSave(next);
    setSaving(false);
    // A failed save keeps the editor open with the text in it.
    if (saved) onDraft((current) => (current === next ? undefined : current));
  }

  return (
    <section className="tsec" aria-labelledby="desc-head">
      <div className="tsec-head">
        <h3 id="desc-head">Description</h3>
        <div className="tsec-tools">
          {editing ? (
            <button
              type="button"
              className="ghost small-btn"
              aria-pressed={source}
              title={source ? "Edit as formatted text" : "Edit the markdown as written"}
              onClick={() => setSource((was) => !was)}
            >
              Markdown source
            </button>
          ) : null}
          <button type="button" className="ghost small-btn" disabled={saving} onClick={() => (editing ? void done() : start())}>
            {editing ? (saving ? "Saving…" : "Done") : "Edit"}
          </button>
        </div>
      </div>
      {editing ? (
        <div
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              void done();
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              void done();
            }
          }}
        >
          {source ? (
            <textarea
              className="desc-source"
              aria-label="Description (markdown)"
              autoFocus
              rows={10}
              value={draft}
              onChange={(event) => {
                typedSource.current = true;
                const value = event.target.value;
                onDraft(() => value);
              }}
            />
          ) : (
            <RichText
              value={draft}
              onChange={(value) => onDraft(() => value)}
              autoFocus
              ariaLabel="Description"
              minHeight={160}
              placeholder="Describe the task. Markdown works."
            />
          )}
        </div>
      ) : (
        <MarkdownView
          className="tpage-desc"
          value={body}
          empty={<p className="dim small">No description yet. Choose Edit to add one. Markdown is supported.</p>}
        />
      )}
    </section>
  );
}

type RunAction = (label: string, action: () => Promise<unknown>, saved?: string) => Promise<boolean>;

function Properties({
  issue,
  agents,
  projects,
  milestones,
  columns,
  busy,
  run,
  save,
}: {
  issue: Issue;
  agents: Agent[];
  projects: Project[];
  milestones: Milestone[];
  columns: Column[];
  busy: string | null;
  run: RunAction;
  save: (patch: Parameters<typeof updateIssue>[1], saved?: string) => Promise<boolean>;
}) {
  const title = (status: string) => columns.find((column) => column.status === status)?.title ?? status.replace(/_/g, " ");
  const moves = (issue.moves ?? []).filter((status) => status !== issue.status);
  const inProject = milestones.filter((milestone) => milestone.projectId === issue.projectId);
  // The server only moves a task between projects while no run is live: a run
  // is working in the old checkout.
  const live = Boolean(issue.currentRun && isLiveRun(issue.currentRun.status));
  const waiting = waitingFor(issue.currentRun, useRunnerOnline());

  return (
    <dl className="props">
      <dt>Status</dt>
      <dd>
        <select
          aria-label="Status"
          value={issue.status}
          disabled={busy !== null || moves.length === 0}
          title={moves.length ? undefined : "The workflow declares no moves from here"}
          onChange={(event) => {
            const status = event.target.value;
            void run("move", () => moveIssue(issue.id, status), `Moved to ${title(status)}`);
          }}
        >
          <option value={issue.status}>{title(issue.status)}</option>
          {moves.map((status) => (
            <option key={status} value={status}>
              {title(status)}
            </option>
          ))}
        </select>
      </dd>

      <dt>Priority</dt>
      <dd>
        <select aria-label="Priority" value={issue.priority} onChange={(event) => void save({ priority: event.target.value })}>
          <PriorityOptions />
        </select>
      </dd>

      <dt>Assignee</dt>
      <dd>
        <select
          aria-label="Assignee"
          value={assigneeKey(issue.assignee)}
          onChange={(event) => void save(parseAssigneeKey(event.target.value))}
        >
          <AssigneeOptions agents={agents} />
        </select>
      </dd>

      <dt>Due</dt>
      <dd className="props-due">
        <DueInput value={issue.dueDate} onSave={(dueDate) => void save({ dueDate })} />
        {issue.dueDate ? (
          <button type="button" className="icon-btn" aria-label="Clear due date" title="Clear due date" onClick={() => void save({ dueDate: null })}>
            ×
          </button>
        ) : null}
      </dd>

      <dt>Project</dt>
      <dd className="props-project">
        <select
          aria-label="Project"
          value={issue.projectId ?? ""}
          disabled={busy !== null || live}
          title={live ? "Wait for the run to finish before moving the task" : undefined}
          onChange={(event) => void save({ projectId: event.target.value || null })}
        >
          {issue.projectId && !projects.some((project) => project.id === issue.projectId) ? (
            <option value={issue.projectId}>{issue.projectName ?? "Project"}</option>
          ) : null}
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
          <option value="">No project</option>
        </select>
        {issue.projectId ? null : <span className="dim small">No project · runs without a checkout</span>}
      </dd>

      <dt>Milestone</dt>
      <dd>
        <select
          aria-label="Milestone"
          value={issue.milestoneId ?? ""}
          onChange={(event) => {
            const milestoneId = event.target.value || null;
            void run(
              "milestone",
              () =>
                gql(`mutation ($id: UUID!, $milestoneId: UUID) { setIssueMilestone(issueId: $id, milestoneId: $milestoneId) { id } }`, {
                  id: issue.id,
                  milestoneId,
                }),
              "Saved",
            );
          }}
        >
          <option value="">None</option>
          {inProject.map((milestone) => (
            <option key={milestone.id} value={milestone.id}>
              {milestone.title}
            </option>
          ))}
        </select>
      </dd>

      <dt>Pull request</dt>
      <dd className="props-text">
        {issue.prUrl ? (
          <a className="chip info" href={issue.prUrl} target="_blank" rel="noreferrer">
            #{issue.prNumber ?? "PR"}
          </a>
        ) : (
          <span className="dim">None linked</span>
        )}
      </dd>

      {issue.currentRun ? (
        <>
          <dt>Run</dt>
          <dd className="props-text">
            <RunStatusChip status={issue.currentRun.status} />
            {waiting ? <span className="run-wait">{waiting}</span> : null}
          </dd>
        </>
      ) : null}

      {issue.turnRemaining?.length ? (
        <>
          <dt>Turns</dt>
          <dd className="props-text">
            <TurnQueue issue={issue} />
          </dd>
        </>
      ) : null}
    </dl>
  );
}

/**
 * A date field that saves once the date is whole: on blur, on Enter, or when
 * the picker hands over a complete date. Typing a year digit by digit does not
 * save four times, and a poll does not reset the field while it has focus.
 */
function DueInput({ value, onSave }: { value: string | null; onSave: (value: string | null) => void }) {
  const [draft, setDraft] = useState(value ?? "");
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setDraft(value ?? "");
  }, [value]);
  const commit = (next: string) => {
    if ((next || null) === (value || null)) return;
    if (next && !/^(19|20|21)\d\d-\d\d-\d\d$/.test(next)) return;
    onSave(next || null);
  };
  return (
    <input
      type="date"
      aria-label="Due date"
      className={draft ? undefined : "date-empty"}
      value={draft}
      onFocus={() => (focused.current = true)}
      onBlur={() => {
        focused.current = false;
        commit(draft);
      }}
      onChange={(event) => {
        setDraft(event.target.value);
        if (!focused.current) commit(event.target.value);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit(draft);
      }}
    />
  );
}

/** The lifecycle buttons the old detail view had, kept with the same rules. */
function Actions({ issue, busy, run }: { issue: Issue; busy: string | null; run: RunAction }) {
  const [sendBack, setSendBack] = useState(false);
  const [changes, setChanges] = useState("");
  const [abandoning, setAbandoning] = useState(false);
  const [reason, setReason] = useState("");
  const live = issue.currentRun && isLiveRun(issue.currentRun.status) ? issue.currentRun : null;
  const verdict = issue.reviews[issue.reviews.length - 1];
  const disabled = busy !== null;
  const limit = issue.runLimit ?? 0;
  const capped = limit > 0 && (issue.runsSinceCap ?? 0) >= limit && !live;

  return (
    <div className="tpage-actions">
      {capped ? (
        <div className="tpage-form" role="status">
          <p className="small">
            This task has used <strong>{issue.runsSinceCap} of {limit} runs</strong>. No agent starts until you allow more.
          </p>
          <button
            type="button"
            className="primary"
            disabled={disabled}
            onClick={() =>
              void run(
                "runcap",
                () => gql(`mutation ($id: UUID!) { clearRunCap(issueId: $id) { id status } }`, { id: issue.id }),
                `Allowed ${limit} more runs`,
              )
            }
          >
            Allow {limit} more runs
          </button>
        </div>
      ) : null}
      {issue.status === "backlog" ? (
        <button
          type="button"
          className="secondary"
          disabled={disabled}
          onClick={() =>
            void run("plan", () => gql(`mutation ($id: UUID!) { planIssue(issueId: $id, planned: true) { id status } }`, { id: issue.id }), "Moved to To do")
          }
        >
          Move to To do
        </button>
      ) : null}
      {issue.status === "todo" && !live ? (
        <button
          type="button"
          className="primary"
          disabled={disabled}
          onClick={() => void run("start", () => gql(`mutation ($id: UUID!) { startRun(issueId: $id) { id status } }`, { id: issue.id }), "Agent started")}
        >
          Start an agent now
        </button>
      ) : null}
      {live ? (
        <button
          type="button"
          className="danger"
          disabled={disabled}
          onClick={() =>
            void run("cancel", () => gql(`mutation ($runId: UUID!) { cancelRun(runId: $runId) { id status } }`, { runId: live.id }), "Run cancelled")
          }
        >
          Cancel the run
        </button>
      ) : null}
      {issue.status === "in_review" ? (
        <>
          {verdict ? (
            <p className="small">
              Reviewer: <strong>{verdict.verdict.replace(/_/g, " ")}</strong>
            </p>
          ) : null}
          <button
            type="button"
            className="primary"
            disabled={disabled}
            onClick={() => void run("merge", () => gql(`mutation ($id: UUID!) { mergeIssue(issueId: $id) { id status } }`, { id: issue.id }), "Marked merged")}
          >
            The pull request merged
          </button>
          {sendBack ? (
            <div className="tpage-form">
              <textarea rows={3} aria-label="What should change" placeholder="What should change?" value={changes} onChange={(event) => setChanges(event.target.value)} />
              <div className="row end">
                <button type="button" className="ghost" onClick={() => setSendBack(false)}>
                  Cancel
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={disabled || !changes.trim()}
                  onClick={() =>
                    void run(
                      "changes",
                      async () => {
                        await gql(`mutation ($id: UUID!, $body: String!) { requestChanges(issueId: $id, body: $body) { id status } }`, {
                          id: issue.id,
                          body: changes.trim(),
                        });
                        setChanges("");
                        setSendBack(false);
                      },
                      "Sent back",
                    )
                  }
                >
                  Send it back
                </button>
              </div>
            </div>
          ) : (
            <button type="button" className="secondary" disabled={disabled} onClick={() => setSendBack(true)}>
              Send it back
            </button>
          )}
        </>
      ) : null}
      {issue.status === "done" || issue.status === "cancelled" ? (
        <button
          type="button"
          className="secondary"
          disabled={disabled}
          onClick={() => void run("reopen", () => gql(`mutation ($id: UUID!) { reopenIssue(issueId: $id) { id status } }`, { id: issue.id }), "Reopened")}
        >
          Reopen
        </button>
      ) : abandoning ? (
        <div className="tpage-form">
          <textarea rows={2} aria-label="Why abandon it" placeholder="Why stop work on this task?" value={reason} onChange={(event) => setReason(event.target.value)} />
          <div className="row end">
            <button type="button" className="ghost" onClick={() => setAbandoning(false)}>
              Keep it
            </button>
            <button
              type="button"
              className="danger"
              disabled={disabled || !reason.trim()}
              onClick={() =>
                void run(
                  "abandon",
                  async () => {
                    await gql(`mutation ($id: UUID!, $reason: String!) { abandonIssue(issueId: $id, reason: $reason) { id status } }`, {
                      id: issue.id,
                      reason: reason.trim(),
                    });
                    setReason("");
                    setAbandoning(false);
                  },
                  "Abandoned",
                )
              }
            >
              Abandon
            </button>
          </div>
        </div>
      ) : (
        <button type="button" className="link danger-link" disabled={disabled} onClick={() => setAbandoning(true)}>
          Abandon this task…
        </button>
      )}
    </div>
  );
}
