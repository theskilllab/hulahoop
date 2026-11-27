import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import {
  addPullRequestComment,
  deletePullRequestBranch,
  deletePullRequestComment,
  loadConnections,
  loadGithubAccount,
  loadPullRequest,
  loadPullRequests,
  loadPullRequestSyncStatus,
  mergePullRequest,
  reviewPullRequest,
  syncPullRequests,
  type CiState,
  type GithubAccount,
  type MergeMethod,
  type PullRequest,
  type PullRequestDetail,
  type PullRequestFilter,
  type PullRequestSyncStatus,
  type ReviewEvent,
} from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import DiffView, { type CommentAnchor } from "../components/DiffView";
import { filesFromGithub, parseGitDiff, totals } from "../components/diff";
import { fmtWhen, plural } from "../components/TaskBits";
import { useToast } from "../components/Toast";
import { useRunDiff } from "../ws";

/** The list refreshes from the server's cache on its own clock, never on the app's 2.5 s one. */
export const LIST_REFRESH_MS = 15_000;

export const FILTERS: { value: PullRequestFilter; label: string }[] = [
  { value: "OPEN", label: "Open" },
  { value: "NEEDS_MY_REVIEW", label: "Needs my review" },
  { value: "MINE", label: "Mine" },
  { value: "BY_AGENTS", label: "By agents" },
  { value: "FAILING_CI", label: "Failing CI" },
  { value: "MERGED", label: "Merged" },
];

export const MERGE_LABEL: Record<MergeMethod, string> = {
  MERGE: "Merge",
  SQUASH: "Squash and merge",
  REBASE: "Rebase and merge",
};

const MERGE_HINT: Record<MergeMethod, string> = {
  MERGE: "Keep every commit, joined by a merge commit",
  SQUASH: "One commit on the base branch",
  REBASE: "Replay the commits onto the base branch",
};

const reason = (cause: unknown) => String(cause instanceof Error ? cause.message : cause);

/** `12 min`, `3 h`, `5 d`: how long ago, as the list shows it. */
export function age(iso: string, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - new Date(iso).getTime()) / 60_000));
  if (!Number.isFinite(minutes)) return "";
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h`;
  return `${Math.round(hours / 24)} d`;
}

export function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  switch (n % 10) {
    case 1:
      return `${n}st`;
    case 2:
      return `${n}nd`;
    case 3:
      return `${n}rd`;
    default:
      return `${n}th`;
  }
}

function stateClass(pr: PullRequest): string {
  if (pr.merged) return "merged";
  if (pr.state === "closed") return "closed";
  return pr.draft ? "draft" : "open";
}

export function ReviewChip({ pr }: { pr: PullRequest }) {
  if (pr.merged || pr.reviewState === "MERGED") return <span className="chip info">Merged</span>;
  if (pr.state === "closed" || pr.reviewState === "CLOSED") return <span className="chip">Closed</span>;
  switch (pr.reviewState) {
    case "DRAFT":
      return <span className="chip">Draft</span>;
    case "REVIEW_REQUESTED": {
      if (pr.changesRequested === 0) return <span className="chip warn">Review requested</span>;
      const round = pr.changesRequested + 1;
      const times = pr.changesRequested === 1 ? "once" : `${pr.changesRequested} times`;
      // Three rounds in, the back-and-forth usually says the task was unclear.
      return (
        <span className={round >= 3 ? "chip bad" : "chip warn"} title={`Sent back for changes ${times}; updated since`}>
          Review requested · {ordinal(round)} round
        </span>
      );
    }
    case "APPROVED":
      return (
        <span className="chip ok">
          <Icon name="check" size={12} />
          Approved
        </span>
      );
    case "CHANGES_REQUESTED":
      return <span className="chip bad">Changes requested</span>;
    default:
      return null;
  }
}

export function CiChip({ state }: { state: CiState }) {
  switch (state) {
    case "PASSING":
      return (
        <span className="chip ok">
          <Icon name="check" size={12} />
          CI passing
        </span>
      );
    case "FAILING":
      return (
        <span className="chip bad">
          <Icon name="close" size={12} />
          CI failing
        </span>
      );
    case "RUNNING":
      return <span className="chip warn">CI running</span>;
    default:
      return null;
  }
}

/** Whether an open pull request has conflicts with its base branch, as GitHub's check last said. */
export function conflicting(pr: PullRequest): boolean {
  return !pr.merged && pr.state === "open" && pr.mergeable === "CONFLICTING";
}

export function ConflictChip({ pr }: { pr: PullRequest }) {
  if (!conflicting(pr)) return null;
  return (
    <span className="chip bad" title={`Conflicts with ${pr.baseRef}`}>
      <Icon name="alert" size={12} />
      Conflicts
    </span>
  );
}

function Stat({ additions, deletions }: { additions: number; deletions: number }) {
  return (
    <span className="mono diff-stat" aria-label={`${additions} added, ${deletions} removed`}>
      <span className="diff-plus">+{additions}</span> <span className="diff-minus">−{deletions}</span>
    </span>
  );
}

/**
 * Pull requests across the workspace's GitHub repositories, from the server's
 * cache: a filtered list beside the one being read, with its diff, line
 * comments, review and merge.
 */
export default function PullRequests({
  onOpenTask,
  onOpenSettings,
  onOpenConnectors,
  onChanged,
}: {
  onOpenTask: (id: string) => void;
  /** Settings → Personal, where the GitHub token goes. */
  onOpenSettings: () => void;
  onOpenConnectors: () => void;
  /** A merge or review moved a task: the app should refresh. */
  onChanged?: () => void;
}) {
  const [filter, setFilter] = useState<PullRequestFilter>("OPEN");
  const [prs, setPrs] = useState<PullRequest[] | null>(null);
  const [sync, setSync] = useState<PullRequestSyncStatus>({ syncedAt: null, problems: [] });
  // undefined: not known yet; null: no token.
  const [account, setAccount] = useState<GithubAccount | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<PullRequest | null>(null);
  const [syncing, setSyncing] = useState(false);
  // Is the workspace's GitHub App connected? undefined: not known.
  const [appConnected, setAppConnected] = useState<boolean | undefined>(undefined);
  // Narrow windows show the list or the one being read, never both.
  const [showDetail, setShowDetail] = useState(false);
  const [toast, showToast] = useToast();
  const ticket = useRef(0);

  /** Resolves false when the list could not be reloaded (the error is shown above the list). */
  const refresh = useCallback(async (): Promise<boolean> => {
    const mine = ++ticket.current;
    try {
      const [list, status, who, connections] = await Promise.all([
        loadPullRequests(filter),
        loadPullRequestSyncStatus(),
        // The account and connections only decide what to suggest; the list does not wait on them.
        loadGithubAccount().catch(() => undefined),
        loadConnections().catch(() => undefined),
      ]);
      if (mine !== ticket.current) return true;
      setPrs(list);
      setSync(status);
      if (who !== undefined) setAccount(who);
      const app = connections?.find((row) => row.id === "github");
      if (app) setAppConnected(app.connected);
      setError(null);
      return true;
    } catch (cause) {
      if (mine !== ticket.current) return true;
      setPrs((current) => current ?? []);
      setError(reason(cause));
      return false;
    }
  }, [filter]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), LIST_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  async function syncNow() {
    setSyncing(true);
    try {
      await syncPullRequests();
      if (await refresh()) showToast("Pull requests are up to date");
      else showToast("Synced with GitHub, but the list could not reload", true);
    } catch (cause) {
      showToast(reason(cause), true);
    } finally {
      setSyncing(false);
    }
  }

  // The selected one, as fresh as the list has it; kept when a filter or a merge drops it from the list.
  const current = (selected && prs?.find((pr) => pr.id === selected.id)) ?? selected;
  const selectedId = useRef<string | null>(null);
  selectedId.current = current?.id ?? null;

  const changed = useCallback(
    (next?: PullRequest) => {
      if (next) {
        setSelected(next);
        setPrs((list) => list?.map((pr) => (pr.id === next.id ? next : pr)) ?? list);
      }
      void refresh();
      onChanged?.();
    },
    [refresh, onChanged],
  );

  const back = useCallback(() => {
    setShowDetail(false);
    // Back on the list, focus returns to the row that was open.
    window.requestAnimationFrame(() => {
      const id = selectedId.current;
      Array.from(document.querySelectorAll<HTMLElement>(".prrow"))
        .find((row) => row.dataset.pr === id)
        ?.focus();
    });
  }, []);

  // Only when nothing can reach GitHub: no token, and the sync reports problems or the App is not connected.
  // A first sync that has not run yet is no reason.
  const noAccess =
    prs !== null &&
    prs.length === 0 &&
    filter === "OPEN" &&
    account === null &&
    (sync.problems.length > 0 || appConnected === false);

  return (
    <section className="screen fill prs">
      <PageHeader
        title="Pull requests"
        subtitle={sync.syncedAt ? `from GitHub · synced ${fmtWhen(sync.syncedAt)}` : "from GitHub"}
        actions={
          <button type="button" className="btn" onClick={() => void syncNow()} disabled={syncing}>
            <Icon name="refresh" size={14} />
            {syncing ? "Syncing…" : "Sync"}
          </button>
        }
      />

      {sync.problems.length > 0 || error ? (
        <div className="pr-problems" role="status">
          <Icon name="alert" size={14} />
          <ul>
            {error ? <li>{error}</li> : null}
            {sync.problems.map((problem) => (
              <li key={problem}>{problem}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {account === null && !noAccess ? (
        <p className="pr-tokenhint small">
          Add your GitHub token to review and merge from here.{" "}
          <button type="button" className="link" onClick={onOpenSettings}>
            Settings → Personal
          </button>
        </p>
      ) : null}

      {noAccess ? (
        <div className="empty pr-noaccess">
          <strong>GitHub isn’t connected</strong>
          Connect GitHub, or add your own GitHub token, to see your pull requests and the diffs agents are working on.
          <span className="od-row pr-noaccess-actions">
            <button type="button" className="primary" onClick={onOpenSettings}>
              <Icon name="gear" size={14} />
              Settings → Personal
            </button>
            <button type="button" className="secondary" onClick={onOpenConnectors}>
              <Icon name="plug" size={14} />
              Open Connectors
            </button>
          </span>
        </div>
      ) : (
        <div className={showDetail && current ? "prsplit showdetail" : "prsplit"}>
          <div className="prlist">
            <div className="pr-filters" role="group" aria-label="Filter">
              {FILTERS.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={filter === option.value}
                  onClick={() => {
                    if (option.value === filter) return;
                    setPrs(null);
                    setFilter(option.value);
                  }}
                >
                  {option.label}
                </button>
              ))}
            </div>
            <div className="pr-rows" aria-busy={prs === null || syncing}>
              {prs === null ? (
                Array.from({ length: 5 }, (_, index) => <div key={index} className="pr-skel" />)
              ) : prs.length === 0 ? (
                <div className="empty">
                  <strong>No pull requests here</strong>
                  Try another filter or sync with GitHub.
                </div>
              ) : (
                prs.map((pr) => (
                  <button
                    key={pr.id}
                    type="button"
                    className="prrow"
                    aria-current={pr.id === current?.id}
                    data-pr={pr.id}
                    onClick={() => {
                      setSelected(pr);
                      setShowDetail(true);
                    }}
                  >
                    <span className={`pr-icon ${stateClass(pr)}`} aria-hidden="true">
                      <Icon name="pr" size={18} />
                    </span>
                    <span className="pr-main">
                      <span className="pr-title clamp-2">{pr.title}</span>
                      <span className="pr-meta">
                        <span>#{pr.number}</span>
                        <span>{pr.repoName.split("/")[1] ?? pr.repoName}</span>
                        <span>
                          {pr.authorLogin}
                          {pr.byAgent ? " (agent)" : ""}
                        </span>
                        <span>{age(pr.updatedAt)}</span>
                      </span>
                    </span>
                    <span className="pr-chips">
                      <ReviewChip pr={pr} />
                      <ConflictChip pr={pr} />
                      {pr.merged ? null : <CiChip state={pr.ciState} />}
                    </span>
                  </button>
                ))
              )}
            </div>
          </div>

          <div className="prdetail">
            {current ? (
              <PullRequestPane
                key={current.id}
                pr={current}
                onOpenTask={onOpenTask}
                onOpenSettings={onOpenSettings}
                onChanged={changed}
                showToast={showToast}
                onBack={back}
              />
            ) : (
              <div className="empty pr-nodetail">
                <strong>Select a pull request</strong>
                Its diff and review actions appear here.
              </div>
            )}
          </div>
        </div>
      )}
      {toast}
    </section>
  );
}

const NO_TOKEN = "Add your GitHub token in Settings → Personal";

function PullRequestPane({
  pr,
  onOpenTask,
  onOpenSettings,
  onChanged,
  showToast,
  onBack,
}: {
  pr: PullRequest;
  onOpenTask: (id: string) => void;
  onOpenSettings: () => void;
  onChanged: (next?: PullRequest) => void;
  showToast: (text: string, bad?: boolean) => void;
  /** Back to the list, on narrow windows. */
  onBack: () => void;
}) {
  const [detail, setDetail] = useState<PullRequestDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // The run being watched, kept after the list stops naming it so "Run finished" and the final diff still show.
  const [watchRun, setWatchRun] = useState<string | null>(null);
  const [feedback, setFeedback] = useState(false);
  const [busy, setBusy] = useState<"approve" | "changes" | "merge" | "branch" | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [method, setMethod] = useState<MergeMethod | null>(null);
  // Only the newest load may land: an older one would bring back comments a review already sent.
  const loadTicket = useRef(0);
  // One review or merge at a time, whoever asks: a second Send must not reach GitHub twice.
  const inFlight = useRef(false);

  const load = useCallback(async () => {
    const mine = ++loadTicket.current;
    try {
      const next = await loadPullRequest(pr.repoId, pr.number);
      if (mine !== loadTicket.current) return;
      setDetail(next);
      setLoadError(next ? null : "GitHub no longer has this pull request.");
    } catch (cause) {
      if (mine === loadTicket.current) setLoadError(reason(cause));
    }
  }, [pr.repoId, pr.number]);

  // Again whenever the list sees the pull request move. CI finishing, a review landing and a conflict appearing do
  // not always move `updatedAt`, so a change in any of the list's checks reloads it too; otherwise the merge button
  // kept saying "CI is still running" until the app was reopened.
  useEffect(() => {
    void load();
  }, [load, pr.updatedAt, pr.headSha, pr.merged, pr.mergeable, pr.ciState, pr.reviewState, pr.changesRequested]);

  const files = useMemo(() => (detail ? filesFromGithub(detail.files) : []), [detail]);
  const sum = useMemo(() => totals(files), [files]);
  const pending = detail ? detail.comments.filter((comment) => comment.pending && !comment.onlyInHulahoop).length : 0;
  const methods = detail?.mergeMethods ?? [];
  const chosen: MergeMethod | null =
    method && methods.includes(method) ? method : detail?.lastMergeMethod && methods.includes(detail.lastMergeMethod) ? detail.lastMergeMethod : methods[0] ?? null;
  const open = !pr.merged && pr.state !== "closed";
  // A draft is read and watched, never reviewed or merged from here.
  const reviewable = open && !pr.draft;
  const liveRunId = pr.liveRun?.id ?? null;
  const canReview = Boolean(detail?.canReview);
  const canMerge = Boolean(detail?.canMerge) && chosen !== null;
  const reviewWhy = detail && !detail.canReview ? detail.reviewBlockedReason ?? "" : "";
  const mergeWhy = detail && !detail.canMerge ? detail.mergeBlockedReason ?? "" : "";
  // The live check when the detail has loaded, the list's until then.
  const conflict = conflicting(detail ? { ...pr, mergeable: detail.pullRequest.mergeable } : pr);
  // The conflict has its own line below the branch, so the merge reason does not say it twice.
  const conflictWhy = conflict && mergeWhy.startsWith("This branch has conflicts");
  const reasons = [...new Set([reviewWhy, conflictWhy ? "" : mergeWhy].filter(Boolean))];
  const task = pr.issue;

  async function review(event: ReviewEvent, note: string | null) {
    if (inFlight.current) return false;
    inFlight.current = true;
    setBusy(event === "APPROVE" ? "approve" : "changes");
    setActionError(null);
    const count = pending;
    try {
      const next = await reviewPullRequest(pr.repoId, pr.number, event, note);
      // A load started before the review would show the sent comments as pending again.
      loadTicket.current += 1;
      setDetail(next);
      setLoadError(null);
      setFeedback(false);
      const tail = count > 0 ? ` · ${plural(count, "comment")}` : "";
      showToast(event === "APPROVE" ? `Approved #${pr.number}${tail}` : `Requested changes on #${pr.number}${tail}`);
      onChanged(next.pullRequest);
      return true;
    } catch (cause) {
      setActionError(reason(cause));
      return false;
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  async function merge() {
    if (!chosen || inFlight.current) return;
    inFlight.current = true;
    setBusy("merge");
    setActionError(null);
    try {
      // The head whose diff is on screen; the list's copy only when the detail has not loaded.
      const merged = await mergePullRequest(pr.repoId, pr.number, chosen, detail?.pullRequest.headSha ?? pr.headSha);
      // Only a task the merge moved to Done is announced as closed.
      const task = merged.issue;
      showToast(task?.status === "done" ? `Merged #${pr.number} and closed ${task.key}` : `Merged #${pr.number}`);
      // The merged pull request moves (merged, updatedAt), and the effect above reloads it once.
      onChanged(merged);
    } catch (cause) {
      setActionError(reason(cause));
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  async function deleteBranch() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy("branch");
    setActionError(null);
    try {
      const next = await deletePullRequestBranch(pr.repoId, pr.number);
      loadTicket.current += 1;
      setDetail(next);
      setLoadError(null);
      showToast(`Deleted branch ${pr.headRef}`);
      onChanged(next.pullRequest);
    } catch (cause) {
      setActionError(reason(cause));
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }

  const { repoId, number } = pr;
  const addComment = useCallback(
    async (anchor: CommentAnchor, body: string) => {
      const saved = await addPullRequestComment({ repoId, number, ...anchor, body });
      // A load started before the save would drop the new comment.
      loadTicket.current += 1;
      setDetail((current) => (current ? { ...current, comments: [...current.comments, saved] } : current));
      showToast(saved.onlyInHulahoop ? "Comment saved in Hulahoop" : "Comment added");
    },
    [repoId, number, showToast],
  );

  const deleteComment = useCallback(async (id: string) => {
    await deletePullRequestComment(id);
    loadTicket.current += 1;
    setDetail((current) => (current ? { ...current, comments: current.comments.filter((comment) => comment.id !== id) } : current));
  }, []);

  return (
    <div className="pr-pane">
      <div className="pr-head">
        <div className="od-row pr-titlerow">
          <span className={`pr-icon ${stateClass(pr)}`} aria-hidden="true">
            <Icon name="pr" size={18} />
          </span>
          <h2 className="od-fill">
            {pr.title} <span className="muted pr-number">#{pr.number}</span>
          </h2>
        </div>
        <div className="pr-cluster">
          <ReviewChip pr={pr} />
          {pr.merged ? null : <CiChip state={pr.ciState} />}
          <span className="chip">{pr.repoName}</span>
          {task ? (
            <button type="button" className="chip info" title={task.title} onClick={() => onOpenTask(task.id)}>
              {task.key}
            </button>
          ) : null}
          <span className="muted pr-by">
            by {pr.authorLogin}
            {pr.byAgent ? " · agent" : ""} · {age(pr.createdAt)}
          </span>
        </div>
        <div className="pr-branch muted mono" title="Head branch into base branch">
          <Icon name="branch" size={12} />
          {pr.headRef} → {pr.baseRef}
        </div>
        {conflict ? (
          <p className="pr-conflict" role="status">
            <Icon name="alert" size={14} />
            This branch has conflicts with {pr.baseRef} that must be resolved.
          </p>
        ) : null}
        <div className="pr-cluster pr-actions">
          <a className="btn" href={pr.url} target="_blank" rel="noreferrer">
            Open on GitHub
          </a>
          {pr.merged && detail?.canDeleteBranch ? (
            <button
              type="button"
              className="link pr-delete-branch"
              disabled={busy !== null}
              title={`Delete ${pr.headRef} on GitHub`}
              onClick={() => void deleteBranch()}
            >
              {busy === "branch" ? "Deleting branch…" : "Delete branch"}
            </button>
          ) : null}
          {liveRunId || watchRun ? (
            <button
              type="button"
              className="btn"
              aria-pressed={watchRun !== null}
              onClick={() => setWatchRun((current) => (current ? null : liveRunId))}
            >
              <Icon name="bot" size={14} />
              {watchRun ? "Show files" : "Watch live"}
            </button>
          ) : null}
          {reviewable && detail ? (
            <>
              <button
                type="button"
                className="btn"
                aria-expanded={feedback}
                disabled={!canReview || busy !== null}
                title={reviewWhy || undefined}
                onClick={() => setFeedback(true)}
              >
                <Icon name="msg" size={14} />
                {pending > 0 ? `Request changes (${pending})` : "Request changes"}
              </button>
              <button
                type="button"
                className="btn"
                disabled={!canReview || busy !== null}
                title={reviewWhy || undefined}
                onClick={() => void review("APPROVE", null)}
              >
                <Icon name="check" size={14} />
                {busy === "approve" ? "Approving…" : "Approve"}
              </button>
              <MergeButton
                methods={methods}
                chosen={chosen}
                onChoose={setMethod}
                disabled={!canMerge || busy !== null}
                busy={busy === "merge"}
                title={mergeWhy}
                onMerge={() => void merge()}
              />
            </>
          ) : null}
          <button type="button" className="btn prback" onClick={onBack}>
            Back
          </button>
        </div>
        {reviewable && reasons.length > 0 ? (
          <p className="pr-why small">
            {reasons.join(" · ")}
            {reasons.includes(NO_TOKEN) ? (
              <>
                {" "}
                <button type="button" className="link" onClick={onOpenSettings}>
                  Open Settings
                </button>
              </>
            ) : null}
          </p>
        ) : null}
        {reviewable && pr.ciState === "FAILING" && !/\bCI\b/.test(mergeWhy) ? (
          <p className="pr-why small">CI is failing on this pull request. You can review it, but merging is blocked until checks pass.</p>
        ) : null}
        {actionError ? (
          <p className="error" role="alert">
            {actionError}
          </p>
        ) : null}
      </div>

      {feedback && detail ? (
        <ReviewBox
          pending={pending}
          busy={busy === "changes"}
          onCancel={() => setFeedback(false)}
          onSend={(note) => review("REQUEST_CHANGES", note)}
        />
      ) : null}

      {watchRun ? (
        <LiveDiff runId={watchRun} />
      ) : (
        <>
          <div className="pr-filesbar">
            <span className="field-label">Files changed · {detail ? files.length : pr.changedFiles}</span>
            <Stat additions={detail ? sum.additions : pr.additions} deletions={detail ? sum.deletions : pr.deletions} />
          </div>
          <div className="pr-body">
            {detail ? (
              <>
                {loadError ? (
                  // A failed background reload keeps what was loaded, and any comment being typed.
                  <p className="error" role="alert">
                    Could not refresh this pull request: {loadError}
                  </p>
                ) : null}
                <DiffView
                  // A new push moves the lines: open comment boxes close rather than land on another line.
                  key={detail.pullRequest.headSha}
                  files={files}
                  comments={detail.comments}
                  onAddComment={open ? addComment : undefined}
                  onDeleteComment={deleteComment}
                />
              </>
            ) : loadError ? (
              <p className="error" role="alert">
                {loadError}
              </p>
            ) : (
              <div className="pr-skel" aria-busy="true" />
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** "What should change?": a note, or line comments, or both. */
export function ReviewBox({
  pending,
  busy,
  onCancel,
  onSend,
}: {
  /** Pending line comments that go with the review. */
  pending: number;
  busy: boolean;
  onCancel: () => void;
  /** Resolves true once sent. */
  onSend: (note: string | null) => Promise<boolean>;
}) {
  const [note, setNote] = useState("");
  const [missing, setMissing] = useState(false);
  const box = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    box.current?.focus();
  }, []);

  function send() {
    if (busy) return;
    if (!note.trim() && pending === 0) {
      setMissing(true);
      box.current?.focus();
      return;
    }
    setMissing(false);
    void onSend(note.trim() || null);
  }

  return (
    <div className="pr-fb">
      <label className="field-label" htmlFor="pr-fb-note">
        What should change?{" "}
        <span className="muted pr-fb-hint">
          {pending > 0
            ? `(${plural(pending, "line comment")} will be sent too)`
            : "(tip: click + beside a line to comment on it)"}
        </span>
      </label>
      <textarea
        id="pr-fb-note"
        ref={box}
        rows={2}
        value={note}
        aria-describedby={missing ? "pr-fb-err" : undefined}
        aria-invalid={missing || undefined}
        onChange={(event) => {
          setNote(event.target.value);
          if (event.target.value.trim()) setMissing(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onCancel();
          } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            send();
          }
        }}
      />
      {missing ? (
        <p className="error" id="pr-fb-err" role="alert">
          Add a note, or comment on a line in the diff, so the change is clear.
        </p>
      ) : null}
      <span className="od-row pr-fb-actions">
        <button type="button" className="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="button" className="primary" onClick={send} disabled={busy}>
          {busy ? "Sending…" : "Send feedback"}
        </button>
      </span>
    </div>
  );
}

/** Merge with the chosen method, or pick another from the menu beside it. */
export function MergeButton({
  methods,
  chosen,
  onChoose,
  disabled,
  busy,
  title,
  onMerge,
}: {
  methods: MergeMethod[];
  chosen: MergeMethod | null;
  onChoose: (method: MergeMethod) => void;
  disabled: boolean;
  busy: boolean;
  /** Why merging is blocked, when it is. */
  title: string;
  onMerge: () => void;
}) {
  const [menu, setMenu] = useState(false);
  const wrap = useRef<HTMLSpanElement>(null);
  const toggle = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!menu) return;
    const close = (event: MouseEvent) => {
      if (!wrap.current?.contains(event.target as Node)) setMenu(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setMenu(false);
        toggle.current?.focus();
      }
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", escape, true);
    wrap.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"][aria-checked="true"]')?.focus();
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", escape, true);
    };
  }, [menu]);

  /** Arrow keys, Home and End move between the methods, as in any menu. */
  function moveInMenu(event: ReactKeyboardEvent<HTMLSpanElement>) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'));
    if (items.length === 0) return;
    event.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? items.length - 1
          : (at + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
    items[next].focus();
  }

  return (
    <span
      className="pr-merge"
      ref={wrap}
      onBlur={(event) => {
        // Tab out of the menu closes it. A click elsewhere is the mousedown listener's job.
        const to = event.relatedTarget as Node | null;
        if (menu && to && !wrap.current?.contains(to)) setMenu(false);
      }}
    >
      <button type="button" className="btn ok pr-merge-main" disabled={disabled} title={title || undefined} onClick={onMerge}>
        {busy ? "Merging…" : chosen ? MERGE_LABEL[chosen] : "Merge"}
      </button>
      {methods.length > 1 ? (
        <button
          ref={toggle}
          type="button"
          className="btn ok pr-merge-more"
          aria-label="Merge method"
          aria-haspopup="menu"
          aria-expanded={menu}
          disabled={disabled}
          title={title || undefined}
          onClick={() => setMenu((on) => !on)}
        >
          <Icon name="chevdown" size={12} />
        </button>
      ) : null}
      {menu ? (
        <span className="pr-merge-menu" role="menu" aria-label="Merge method" onKeyDown={moveInMenu}>
          {methods.map((method) => (
            <button
              key={method}
              type="button"
              role="menuitemradio"
              tabIndex={-1}
              aria-checked={method === chosen}
              onClick={() => {
                onChoose(method);
                setMenu(false);
                toggle.current?.focus();
              }}
            >
              <span className="pr-merge-check" aria-hidden="true">
                {method === chosen ? <Icon name="check" size={12} /> : null}
              </span>
              <span>
                <strong>{MERGE_LABEL[method]}</strong>
                <span className="muted small">{MERGE_HINT[method]}</span>
              </span>
            </button>
          ))}
        </span>
      ) : null}
    </span>
  );
}

/** The run's working diff as the agent writes it. */
function LiveDiff({ runId }: { runId: string }) {
  const { patch, live, updatedAt, error, reconnecting } = useRunDiff(runId);
  const files = useMemo(() => parseGitDiff(patch), [patch]);
  const sum = totals(files);
  return (
    <>
      <div className="pr-filesbar">
        <span className="field-label">Live diff · {files.length}</span>
        <Stat additions={sum.additions} deletions={sum.deletions} />
        <span className={live && updatedAt && !reconnecting ? "chip ok pr-live" : "chip pr-live"} role="status">
          {live && updatedAt && !reconnecting ? <span className="pulse" aria-hidden="true" /> : null}
          {reconnecting
            ? "Reconnecting…"
            : !updatedAt
              ? error
                ? "Not connected"
                : "Connecting…"
              : live
                ? "Live"
                : "Run finished"}
        </span>
      </div>
      <div className="pr-body" aria-live="off">
        {error ? (
          <p className="error" role="alert">
            {error}
          </p>
        ) : updatedAt ? (
          <DiffView files={files} empty="The agent has not changed any files yet" />
        ) : (
          <div className="pr-skel" aria-busy="true" />
        )}
      </div>
    </>
  );
}
