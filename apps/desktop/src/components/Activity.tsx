import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { addComment, gql, type Agent, type Comment, type Issue, type IssueStats, type Run } from "../api";
import MarkdownView from "./MarkdownView";
import MentionEditor, { type MentionCandidate } from "./MentionEditor";
import Seg from "./Seg";
import { Avatar, RunStatusChip, fmtDuration, fmtTokens, fmtUsd, fmtWhen, isLiveRun, plural } from "./TaskBits";

/** The REL-104 strip: what this task has cost across every agent and run. */
export function EffortStats({ stats }: { stats: IssueStats }) {
  if (!stats.runs) return null;
  const perTurn = stats.turns ? (stats.toolCalls / stats.turns).toFixed(1) : null;
  return (
    <dl className="effort" aria-label="Effort on this task">
      <div>
        <dt>Runs</dt>
        <dd>
          {stats.runs}
          {stats.failedRuns ? <span className="bad"> · {stats.failedRuns} failed</span> : null}
        </dd>
      </div>
      <div>
        <dt>Turns</dt>
        <dd>{stats.turns}</dd>
      </div>
      <div>
        <dt>Tool calls</dt>
        <dd>
          {stats.toolCalls}
          {perTurn ? <span className="dim"> · {perTurn} per turn</span> : null}
        </dd>
      </div>
      <div>
        <dt>Tokens</dt>
        <dd>{fmtTokens(stats.tokens)}</dd>
      </div>
      <div>
        <dt>Cost</dt>
        <dd>
          {fmtUsd(stats.apiEquivalentUsd)} <span className="dim">at API rates</span>
        </dd>
      </div>
    </dl>
  );
}

function commentKind(comment: Comment): "agent" | "lumi" | "server" | "human" {
  if (comment.authorType === "agent") return "agent";
  if (comment.authorType === "server") return comment.author === "Lumi" ? "lumi" : "server";
  return "human";
}

const CHIP: Record<string, string | null> = { agent: "agent", lumi: "assistant", server: "server", human: null };

/// A long comment shows its first lines; `.cm-clamp` in styles.css sets the
/// same numbers (6 lines of 1.55) as its `max-height`.
const CLAMP_LINES = 6;
const CLAMP_LINE_HEIGHT = 1.55;

/**
 * A comment body cut to its first few lines, with a Show more / Show less
 * toggle that appears only when the text is actually longer than that. It
 * measures again whenever the column or the content changes size, including
 * when MarkdownView swaps its root element. With `clamp` off (a pinned
 * question) the whole text always shows.
 */
function ClampedMarkdown({ value, mentions, clamp = true }: { value: string; mentions: string[]; clamp?: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const id = useId();
  const [open, setOpen] = useState(false);
  const [overflows, setOverflows] = useState(false);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el || !clamp) {
      setOverflows(false);
      return;
    }
    const measure = () => {
      const limit = parseFloat(getComputedStyle(el).fontSize) * CLAMP_LINE_HEIGHT * CLAMP_LINES;
      setOverflows(el.scrollHeight > limit + 1);
    };
    // The box itself stops growing once clamped, so its content's root is
    // watched too, and watched afresh whenever that root is replaced.
    const resize = new ResizeObserver(measure);
    let root: Element | null = null;
    const attach = () => {
      if (el.firstElementChild === root) return;
      if (root) resize.unobserve(root);
      root = el.firstElementChild;
      if (root) resize.observe(root);
      measure();
    };
    resize.observe(el);
    attach();
    measure();
    const swaps = new MutationObserver(attach);
    swaps.observe(el, { childList: true });
    return () => {
      resize.disconnect();
      swaps.disconnect();
    };
  }, [value, clamp]);

  const clamped = overflows && !open;
  return (
    <>
      <div
        ref={box}
        id={id}
        className={`cm-clamp${clamped ? " clamped" : ""}`}
        // A link tabbed to in the hidden part opens the text, so keyboard
        // focus never lands somewhere out of sight.
        onFocus={clamped ? (event) => event.target.matches(":focus-visible") && setOpen(true) : undefined}
      >
        <MarkdownView className="cm-body" value={value} mentions={mentions} />
      </div>
      {overflows ? (
        <button
          type="button"
          className="cm-more"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen((shown) => !shown)}
        >
          {open ? "Show less" : "Show more"}
        </button>
      ) : null}
    </>
  );
}

export function CommentItem({
  comment,
  mentions,
  onOpenRun,
}: {
  comment: Comment;
  mentions: string[];
  onOpenRun?: (runId: string) => void;
}) {
  const kind = commentKind(comment);
  const name = kind === "human" ? (comment.author === "you" ? "You" : comment.author) : comment.author;
  if (kind === "server") {
    return (
      <li className="act cm-note">
        <MarkdownView value={comment.body} mentions={mentions} />
        <time className="dim">{fmtWhen(comment.createdAt)}</time>
      </li>
    );
  }
  return (
    <li className={`act${comment.pinned ? " pinned" : ""}`} id={`comment-${comment.id}`}>
      <Avatar name={name} kind={kind} />
      <div className="act-main">
        <div className="act-head">
          <strong>{name}</strong>
          {CHIP[kind] ? <span className="chip">{CHIP[kind]}</span> : null}
          {comment.pinned ? <span className="chip warn">question</span> : null}
          <time className="dim">{fmtWhen(comment.createdAt)}</time>
          {comment.runId && onOpenRun ? (
            <button type="button" className="linkbtn" onClick={() => onOpenRun(comment.runId as string)}>
              Open transcript
            </button>
          ) : null}
        </div>
        <ClampedMarkdown value={comment.body} mentions={mentions} clamp={!comment.pinned} />
      </div>
    </li>
  );
}

export function RunCard({
  run,
  agentName,
  showSummary = true,
  onOpenTranscript,
}: {
  run: Run;
  agentName: string;
  /** Off when the agent posted the summary as its own comment, so it shows once. */
  showSummary?: boolean;
  onOpenTranscript: (run: Run) => void;
}) {
  const live = isLiveRun(run.status);
  const meta = [
    run.actualModel || run.model || null,
    run.modelTurns ? plural(run.modelTurns, "turn") : null,
    run.toolCalls ? plural(run.toolCalls, "tool call") : null,
    fmtDuration(run.startedAt, run.finishedAt),
    run.tokensIn + run.tokensOut ? `${fmtTokens(run.tokensIn + run.tokensOut)} tokens` : null,
    run.apiEquivalentUsd ? `${fmtUsd(run.apiEquivalentUsd)} at API rates` : null,
  ].filter(Boolean);
  return (
    <li className={`act run-card${live ? " live" : ""}`} id={`run-${run.id}`}>
      <Avatar name={agentName} kind="agent" />
      <div className="act-main">
        <div className="act-head">
          <strong>{agentName}</strong>
          <span className="dim">· run {run.attempt} · {fmtWhen(run.startedAt ?? run.createdAt)}</span>
          <RunStatusChip status={run.status} />
        </div>
        {showSummary && run.summary ? <MarkdownView className="run-sum" value={run.summary} /> : null}
        {run.error && (run.status === "failed" || run.status === "timed_out") ? (
          <p className="run-why">
            {run.failureCategory ? <strong>{run.failureCategory.replace(/_/g, " ")}: </strong> : null}
            {run.error}
          </p>
        ) : null}
        {meta.length ? <p className="run-meta">{meta.join(" · ")}</p> : null}
        <div className="row">
          <button type="button" className="secondary small-btn" onClick={() => onOpenTranscript(run)}>
            {live ? (
              <>
                <span className="pulse" aria-hidden="true" /> Follow live
              </>
            ) : (
              "Open transcript"
            )}
          </button>
          {run.prNumber && run.prUrl ? (
            <a className="chip info" href={run.prUrl} target="_blank" rel="noreferrer">
              PR #{run.prNumber}
            </a>
          ) : null}
        </div>
      </div>
    </li>
  );
}

/** A blocked task's pinned question, with the answer box that resumes it. */
function BlockedQuestion({ issue, question, onChanged, onError }: {
  issue: Issue;
  question: Comment | null;
  onChanged: () => void | Promise<void>;
  onError: (message: string) => void;
}) {
  const [answer, setAnswer] = useState("");
  const [busy, setBusy] = useState(false);
  async function resume() {
    if (!answer.trim()) return;
    setBusy(true);
    try {
      await gql(`mutation ($id: UUID!, $answer: String!) { resumeIssue(issueId: $id, answer: $answer) { id } }`, {
        id: issue.id,
        answer: answer.trim(),
      });
      setAnswer("");
      await onChanged();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="blocked-q" role="region" aria-label="Question from the agent">
      <div className="act-head">
        <span className="chip bad">Blocked</span>
        <strong>{question ? `${question.author} asks` : "This task is waiting for an answer"}</strong>
      </div>
      {question ? <MarkdownView className="cm-body" value={question.body} /> : issue.blockedBy ? <p>{issue.blockedBy}</p> : null}
      <textarea
        rows={3}
        value={answer}
        aria-label="Your answer"
        placeholder="Answer, and the agent picks the task back up."
        onChange={(event) => setAnswer(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void resume();
          }
        }}
      />
      <div className="row end">
        <button type="button" className="primary" disabled={busy || !answer.trim()} onClick={() => void resume()}>
          Answer and resume
        </button>
      </div>
    </div>
  );
}

type Filter = "all" | "comments" | "runs";

/**
 * The task's activity: the effort strip, a composer that can `@` agents and
 * Lumi, and one newest-first stream of comments and run cards.
 */
export default function Activity({
  issue,
  agents,
  draft,
  onDraft,
  onOpenTranscript,
  onChanged,
  onError,
}: {
  issue: Issue;
  agents: Agent[];
  /** The unsent comment; the task screen keeps it per task. */
  draft: string;
  onDraft: (update: (current: string) => string) => void;
  onOpenTranscript: (run: Run) => void;
  onChanged: () => void | Promise<void>;
  onError: (message: string) => void;
}) {
  const [filter, setFilter] = useState<Filter>("all");
  const [sending, setSending] = useState(false);

  const agentName = useMemo(() => new Map(agents.map((agent) => [agent.id, agent.name])), [agents]);
  const candidates: MentionCandidate[] = useMemo(
    () => [
      ...agents.map((agent) => ({ id: agent.id, name: agent.name, kind: "agent" as const })),
      { id: "lumi", name: "Lumi", kind: "lumi" as const },
    ],
    [agents],
  );
  const mentionNames = useMemo(() => candidates.map((entry) => entry.name), [candidates]);

  const question = issue.comments.find((comment) => comment.id === issue.pinnedCommentId) ?? null;
  const entries = useMemo(() => {
    const list: { at: string; comment?: Comment; run?: Run }[] = [];
    if (filter !== "runs") for (const comment of issue.comments) list.push({ at: comment.createdAt, comment });
    if (filter !== "comments") for (const run of issue.runs) list.push({ at: run.startedAt ?? run.createdAt ?? "", run });
    return list.sort((a, b) => b.at.localeCompare(a.at));
  }, [issue.comments, issue.runs, filter]);
  // Runs whose summary the agent also posted as a comment: the comment carries
  // the text (and its Open transcript link), the run card only the run. A
  // pinned question or a server note from the run is not its summary.
  const commented = useMemo(
    () =>
      new Set(
        issue.comments.flatMap((comment) =>
          comment.runId && comment.authorType === "agent" && !comment.pinned ? [comment.runId] : [],
        ),
      ),
    [issue.comments],
  );

  async function send() {
    const sent = draft;
    const body = sent.trim();
    if (!body || sending) return;
    setSending(true);
    try {
      await addComment(issue.id, body);
      // Only clear what was sent: text typed while the comment was on its way stays.
      onDraft((current) => (current === sent ? "" : current));
      await onChanged();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSending(false);
    }
  }

  const openRun = (runId: string) => {
    const run = issue.runs.find((entry) => entry.id === runId);
    if (run) onOpenTranscript(run);
  };

  return (
    <section className="tsec activity" aria-labelledby="activity-head">
      <div className="tsec-head">
        <h3 id="activity-head">
          Activity{" "}
          <span className="dim">
            · {plural(issue.comments.length, "comment")}, {plural(issue.runs.length, "run")}
          </span>
        </h3>
        <Seg<Filter>
          small
          label="Show"
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: "All" },
            { value: "comments", label: "Comments" },
            { value: "runs", label: "Agent runs" },
          ]}
        />
      </div>
      {issue.stats ? <EffortStats stats={issue.stats} /> : null}
      {issue.status === "blocked" ? <BlockedQuestion issue={issue} question={question} onChanged={onChanged} onError={onError} /> : null}
      <div className="composer">
        <MentionEditor value={draft} onChange={(value) => onDraft(() => value)} candidates={candidates} onSubmit={() => void send()} />
        <div className="composer-foot">
          <span className="dim small">⌘↵ to send · @ to mention an agent or Lumi</span>
          <button type="button" className="primary" disabled={sending || !draft.trim()} onClick={() => void send()}>
            Comment
          </button>
        </div>
      </div>
      {entries.length ? (
        <ol className="act-list">
          {entries.map((entry) =>
            entry.comment ? (
              <CommentItem key={`c-${entry.comment.id}`} comment={entry.comment} mentions={mentionNames} onOpenRun={openRun} />
            ) : entry.run ? (
              <RunCard
                key={`r-${entry.run.id}`}
                run={entry.run}
                agentName={agentName.get(entry.run.agentId) ?? "Agent"}
                showSummary={filter === "runs" || !commented.has(entry.run.id)}
                onOpenTranscript={onOpenTranscript}
              />
            ) : null,
          )}
        </ol>
      ) : (
        <p className="dim small">Nothing yet. Comments and agent runs show up here, newest first.</p>
      )}
    </section>
  );
}
