import { useState } from "react";
import { gql, type Issue, type Milestone } from "../api";
import { StatusPill } from "./Board";

interface Props {
  /** The milestones this workspace knows about, so a task can be filed under one. */
  milestones: Milestone[];
  issue: Issue;
  onBack: () => void;
  onChanged: () => Promise<void> | void;
}

const ROLE_LABEL: Record<string, string> = {
  test_author: "Test author",
  implementer: "Agent",
  reviewer: "Reviewer",
};

/**
 * One task, and the moves it allows.
 *
 * The board is the product; this drawer is where a task is read and where the
 * three moves a person still makes by hand live: out of the backlog, back from
 * review, and the merge that closes it.
 */
export default function IssueDetail({ milestones, issue, onBack, onChanged }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  const [comment, setComment] = useState("");
  const [openLog, setOpenLog] = useState<string | null>(null);
  const [log, setLog] = useState<string | null>(null);

  const runs = issue.runs ?? [];
  const current = issue.currentRun ?? runs[runs.length - 1] ?? null;
  const live = current && (current.status === "running" || current.status === "queued");
  const verdict = current ? issue.reviews?.filter((r) => r.reviewedRunId === current.id).slice(-1)[0] : undefined;

  async function run<T>(label: string, query: string, variables: Record<string, unknown>): Promise<T | null> {
    setBusy(label);
    setError(null);
    try {
      const data = await gql<T>(query, variables);
      await onChanged();
      return data;
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
      return null;
    } finally {
      setBusy(null);
    }
  }

  /** Every move is a full document, so `just check-ui` can verify each one. */
  const mutate = (label: string, document: string, variables: Record<string, unknown>) =>
    run(label, document, variables);

  async function showLog(id: string) {
    if (openLog === id) {
      setOpenLog(null);
      return;
    }
    setOpenLog(id);
    setLog(null);
    const data = await run<{ run: { log: string } }>("log", `query ($id: UUID!) { run(id: $id) { log } }`, { id });
    setLog(data?.run.log ?? "no transcript was recorded for this run");
  }

  return (
    <div className="detail">
      <div className="detail-head">
        <div className="row">
          <button className="ghost" onClick={onBack}>
            ← Board
          </button>
          <span className="dim" style={{ marginLeft: "auto" }}>
            {issue.key}
          </span>
          <StatusPill status={issue.status} />
        </div>
        <h1>{issue.title}</h1>
        <div className="meta">
          {issue.projectName ? <span>{issue.projectName}</span> : <span>no project</span>}
          <span>·</span>
          <span>attempt {issue.attempt}</span>
          {issue.prUrl ? (
            <>
              <span>·</span>
              <a className="pr-link" href={issue.prUrl} target="_blank" rel="noreferrer">
                pull request #{issue.prNumber}
              </a>
            </>
          ) : null}
        </div>
      </div>

      {error && <p className="error">{error}</p>}

      {issue.blockedBy ? (
        <div className="blocked-box">
          <strong>Blocked.</strong> {issue.blockedBy}
        </div>
      ) : null}

      <div className="actions">
        {issue.status === "backlog" && (
          <button
            className="primary"
            disabled={busy !== null}
            onClick={() =>
              mutate(
                "plan",
                `mutation ($id: UUID!) { planIssue(issueId: $id, planned: true) { id status } }`,
                { id: issue.id },
              )
            }
          >
            Move to To do
          </button>
        )}

        {issue.status === "todo" && !live && (
          <button
            className="primary"
            disabled={busy !== null}
            onClick={() =>
              mutate(
                "start",
                `mutation ($id: UUID!) { startRun(issueId: $id, role: "implementer") { id status } }`,
                { id: issue.id },
              )
            }
          >
            Start an agent now
          </button>
        )}

        {live && (
          <button
            className="danger"
            disabled={busy !== null}
            onClick={() =>
              mutate(
                "cancel",
                `mutation ($runId: UUID!) { cancelRun(runId: $runId) { id status } }`,
                { runId: current!.id },
              )
            }
          >
            Cancel the run
          </button>
        )}

        {issue.status === "in_review" && (
          <>
            <button
              className="primary"
              disabled={busy !== null}
              onClick={() =>
              mutate("merge", `mutation ($id: UUID!) { mergeIssue(issueId: $id) { id status } }`, { id: issue.id })
            }
            >
              The pull request merged
            </button>
            <button
              disabled={busy !== null}
              onClick={() =>
                mutate(
                  "changes",
                  `mutation ($id: UUID!) { requestChanges(issueId: $id) { id status } }`,
                  { id: issue.id },
                )
              }
            >
              Send it back
            </button>
          </>
        )}

        {issue.status === "done" && (
          <button
            disabled={busy !== null}
            onClick={() =>
              mutate("reopen", `mutation ($id: UUID!) { reopenIssue(issueId: $id) { id status } }`, { id: issue.id })
            }
          >
            Reopen
          </button>
        )}

        <label className="project-pick" style={{ marginLeft: "auto" }}>
          <span>milestone</span>
          <select
            value={issue.milestoneId ?? ""}
            onChange={(event) =>
              void run(
                "milestone",
                `mutation ($id: UUID!, $milestoneId: UUID) { setIssueMilestone(issueId: $id, milestoneId: $milestoneId) { id } }`,
                { id: issue.id, milestoneId: event.target.value || null },
              )
            }
          >
            <option value="">none</option>
            {milestones.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.title}
              </option>
            ))}
          </select>
        </label>
      </div>

      {issue.status === "blocked" && (
        <div className="panel">
          <h3>Resume</h3>
          <textarea
            rows={2}
            placeholder="what the agent should know before it continues"
            value={answer}
            onChange={(event) => setAnswer(event.target.value)}
          />
          <button
            className="primary"
            disabled={busy !== null || answer.trim().length === 0}
            onClick={() =>
              void run(
                "resume",
                `mutation ($id: UUID!, $answer: String!) { resumeIssue(issueId: $id, answer: $answer) { id } }`,
                { id: issue.id, answer },
              ).then(() => setAnswer(""))
            }
          >
            Resume
          </button>
        </div>
      )}

      {issue.body ? (
        <div className="section">
          <h3>Description</h3>
          <p style={{ whiteSpace: "pre-wrap" }}>{issue.body}</p>
        </div>
      ) : null}

      <div className="section">
        <h3>Acceptance criteria</h3>
        {(issue.criteria ?? []).length === 0 ? (
          <p className="dim">No criteria yet. Each line becomes one test the agent has to make pass.</p>
        ) : (
          <ul className="criteria">
            {(issue.criteria ?? []).map((criterion) => (
              <li key={criterion.id}>
                <span>
                  <span className="ac">ac{criterion.position}</span>
                  {criterion.text}
                </span>
                {criterion.testName ? <span className="dim">{criterion.testName}</span> : null}
              </li>
            ))}
          </ul>
        )}
      </div>

      {verdict ? (
        <div className="section">
          <h3>
            Reviewer verdict: <span className={verdict.verdict === "approved" ? "chip ok" : "chip bad"}>{verdict.verdict}</span>
          </h3>
          <pre className="review-body">{verdict.body}</pre>
        </div>
      ) : null}

      <div className="section">
        <h3>Runs</h3>
        {runs.length === 0 ? (
          <p className="dim">Nothing has run yet.</p>
        ) : (
          <ul className="runs">
            {[...runs].reverse().map((entry) => (
              <li key={entry.id}>
                <div className="row">
                  <span className="role">{ROLE_LABEL[entry.role] ?? entry.role}</span>
                  <StatusPill status={entry.status} />
                  <span className="dim">attempt {entry.attempt}</span>
                  {entry.adapter ? <span className="mark">{entry.adapter}</span> : null}
                  <span className="dim" style={{ marginLeft: "auto" }}>
                    {entry.startedAt ? new Date(entry.startedAt).toLocaleString() : ""}
                  </span>
                  <button className="link" onClick={() => void showLog(entry.id)}>
                    {openLog === entry.id ? "hide the transcript" : "transcript"}
                  </button>
                </div>
                {entry.error ? <p className="error">{entry.error}</p> : null}
                {entry.commitName ? <span className="dim">committed as {entry.commitName}</span> : null}
                {openLog === entry.id ? <pre className="log">{log ?? "reading…"}</pre> : null}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="section">
        <h3>Comments</h3>
        <ul className="comments">
          {(issue.comments ?? []).map((entry) => (
            <li key={entry.id}>
              <span className="dim">
                {entry.author} · {new Date(entry.createdAt).toLocaleString()}
              </span>
              <span style={{ whiteSpace: "pre-wrap" }}>{entry.body}</span>
            </li>
          ))}
        </ul>
        <textarea
          rows={2}
          placeholder="a note for whoever picks this up"
          value={comment}
          onChange={(event) => setComment(event.target.value)}
        />
        <button
          disabled={busy !== null || comment.trim().length === 0}
          onClick={() =>
            void run(
              "comment",
              `mutation ($id: UUID!, $body: String!) { addComment(issueId: $id, body: $body) { id } }`,
              { id: issue.id, body: comment },
            ).then(() => setComment(""))
          }
        >
          Comment
        </button>
      </div>
    </div>
  );
}
