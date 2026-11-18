import { useState } from "react";
import { gql, type Issue } from "../api";
import { StatusPill } from "./Board";

interface Props {
  issue: Issue;
  onBack: () => void;
  onChanged: () => Promise<void> | void;
}

const ROLE_LABEL: Record<string, string> = {
  test_author: "Test Author",
  implementer: "Implementer",
  reviewer: "Reviewer",
};

export default function IssueDetail({ issue, onBack, onChanged }: Props) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showDiff, setShowDiff] = useState(false);
  const [approving, setApproving] = useState(false);
  const [ackFlags, setAckFlags] = useState<string[]>([]);
  const [answer, setAnswer] = useState("");
  const [reason, setReason] = useState("");
  const [mergeRevision, setMergeRevision] = useState("");

  const current = issue.currentRun ?? issue.runs[issue.runs.length - 1] ?? null;

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

  const startRun = (role: string) =>
    run("start", `mutation ($issueId: UUID!, $role: String!) { startRun(issueId: $issueId, role: $role) { id } }`, {
      issueId: issue.id,
      role,
    });

  return (
    <section className="detail">
      <header className="detail-head">
        <button className="ghost" onClick={onBack}>
          ← board
        </button>
        <div className="titles">
          <h1>{issue.title}</h1>
          <div className="meta">
            <StatusPill status={issue.status} />
            <span className="dim">
              attempt {issue.attempt} · {issue.runCount} runs · {issue.criteria.length} criteria
            </span>
          </div>
        </div>
      </header>

      {error && <p className="error">{error}</p>}

      <ol className="criteria">
        {issue.criteria.map((criterion) => (
          <li key={criterion.position} className={criterion.testName ? "proven" : ""}>
            <span className="ac">ac{criterion.position}</span>
            <span>{criterion.text}</span>
            <code>{criterion.testName ?? "no test yet"}</code>
          </li>
        ))}
      </ol>

      {issue.body && <p className="body">{issue.body}</p>}

      <div className="actions">
        {issue.status === "backlog" && (
          <button
            className="primary"
            disabled={busy !== null}
            onClick={() => run("ready", `mutation ($id: UUID!) { approveCriteria(issueId: $id) { id } }`, { id: issue.id })}
          >
            Approve criteria
          </button>
        )}
        {issue.status === "ready" && (
          <button className="primary" disabled={busy !== null} onClick={() => startRun("test_author")}>
            Write the tests
          </button>
        )}
        {issue.status === "tests_in_progress" && (
          <>
            {current && <RunSummary issue={issue} run={current} onToggleDiff={() => setShowDiff(!showDiff)} showDiff={showDiff} />}
            {current && current.status === "succeeded" && (
              <button
                className="primary"
                disabled={busy !== null}
                onClick={() => run("freeze", `mutation ($id: UUID!) { freezeSuite(issueId: $id) { id } }`, { id: issue.id })}
              >
                Freeze suite
              </button>
            )}
            {(!current || (current.status !== "running" && current.status !== "queued" && current.status !== "succeeded")) && (
              <button className="ghost" disabled={busy !== null} onClick={() => startRun("test_author")}>
                Write the tests again
              </button>
            )}
          </>
        )}
        {issue.status === "tests_ready" && (
          <button className="primary" disabled={busy !== null} onClick={() => startRun("implementer")}>
            Implement
          </button>
        )}
        {issue.status === "in_progress" && (
          <>
            {current && <RunSummary issue={issue} run={current} onToggleDiff={() => setShowDiff(!showDiff)} showDiff={showDiff} />}
            {current && (current.status === "running" || current.status === "queued") && (
              <button
                className="danger"
                disabled={busy !== null}
                onClick={() =>
                  run("cancel", `mutation ($runId: UUID!) { cancelRun(runId: $runId) { id } }`, { runId: current.id })
                }
              >
                Cancel run
              </button>
            )}
          </>
        )}
        {issue.status === "in_review" && current && (
          <>
            <RunSummary issue={issue} run={current} onToggleDiff={() => setShowDiff(!showDiff)} showDiff={showDiff} />
            {issue.reviews.length === 0 && (
              <button className="primary" disabled={busy !== null} onClick={() => startRun("reviewer")}>
                Ask the Reviewer
              </button>
            )}
            {issue.reviews.length > 0 && (
              <>
                <h3>Reviewer verdict: {issue.reviews[issue.reviews.length - 1].verdict}</h3>
                <pre className="review-body">{issue.reviews[issue.reviews.length - 1].body}</pre>
                {issue.reviews[issue.reviews.length - 1].verdict === "approved" && (
                  <button className="primary" onClick={() => setApproving(true)}>
                    Approve…
                  </button>
                )}
              </>
            )}
          </>
        )}
        {issue.status === "merging" && (
          <div className="merge">
            <p>
              The landing replay is green. Merge the approved head in your checkout, then record the merge commit here — Hulahoop
              verifies its ancestry before closing the issue.
            </p>
            <input placeholder="merge commit revision" value={mergeRevision} onChange={(e) => setMergeRevision(e.target.value)} />
            <button
              className="primary"
              disabled={busy !== null || mergeRevision.trim().length < 4}
              onClick={() =>
                run(
                  "merge",
                  `mutation ($id: UUID!, $rev: String!) { recordMerge(issueId: $id, mergeRevision: $rev) { id } }`,
                  { id: issue.id, rev: mergeRevision.trim() },
                )
              }
            >
              Record merge
            </button>
          </div>
        )}
        {issue.status === "blocked" && (
          <div className="blocked-box">
            <h3>The agent is blocked</h3>
            <p>{issue.comments.find((c) => c.id === issue.pinnedCommentId)?.body ?? "No question recorded."}</p>
            <textarea value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Answer the question…" rows={3} />
            <button
              className="primary"
              disabled={busy !== null || answer.trim().length === 0}
              onClick={() =>
                run("resume", `mutation ($id: UUID!, $answer: String!) { resumeIssue(issueId: $id, answer: $answer) { id } }`, {
                  id: issue.id,
                  answer: answer.trim(),
                })
              }
            >
              Resume
            </button>
          </div>
        )}
        {(issue.status === "in_review" || issue.status === "in_progress") && (
          <details className="secondary">
            <summary>More</summary>
            <div className="row">
              <input placeholder="reason to thaw the suite" value={reason} onChange={(e) => setReason(e.target.value)} />
              <button
                disabled={busy !== null || reason.trim().length === 0}
                onClick={() =>
                  run(
                    "thaw",
                    `mutation ($id: UUID!, $reason: String!) { unfreezeSuite(issueId: $id, reason: $reason) { id } }`,
                    { id: issue.id, reason: reason.trim() },
                  )
                }
              >
                Thaw suite
              </button>
            </div>
            <div className="row">
              <input placeholder="reason to abandon" value={reason} onChange={(e) => setReason(e.target.value)} />
              <button
                className="danger"
                disabled={busy !== null || reason.trim().length === 0}
                onClick={() =>
                  run(
                    "abandon",
                    `mutation ($id: UUID!, $reason: String!) { abandonIssue(issueId: $id, reason: $reason) { id } }`,
                    { id: issue.id, reason: reason.trim() },
                  )
                }
              >
                Abandon issue
              </button>
            </div>
          </details>
        )}
      </div>

      {approving && current && (
        <div className="dialog">
          <h2>Approve the diff you reviewed</h2>
          <p className="dim">
            head {current.headRevision?.slice(0, 12)} · diff {current.diffSha?.slice(0, 12)}
          </p>
          {current.flags.length > 0 ? (
            <ul className="flags">
              {current.flags.map((flag) => (
                <li key={flag}>
                  <label>
                    <input
                      type="checkbox"
                      checked={ackFlags.includes(flag)}
                      onChange={(e) =>
                        setAckFlags(e.target.checked ? [...ackFlags, flag] : ackFlags.filter((f) => f !== flag))
                      }
                    />
                    {flag.replaceAll("_", " ").toLowerCase()} — I read this part of the diff.
                  </label>
                </li>
              ))}
            </ul>
          ) : (
            <p>No flags on this run.</p>
          )}
          <div className="row">
            <button className="ghost" onClick={() => setApproving(false)}>
              Cancel
            </button>
            <button
              className="primary"
              disabled={busy !== null || ackFlags.length !== current.flags.length}
              onClick={async () => {
                await run(
                  "approve",
                  `mutation ($issueId: UUID!, $runId: UUID!, $head: String!, $sha: String!, $flags: [String!]!) {
                     approveIssue(issueId: $issueId, runId: $runId, reviewedHead: $head, diffSha: $sha, acknowledgeFlags: $flags) { id }
                   }`,
                  { issueId: issue.id, runId: current.id, head: current.headRevision ?? "", sha: current.diffSha ?? "", flags: ackFlags },
                );
                setApproving(false);
              }}
            >
              Approve this diff
            </button>
          </div>
        </div>
      )}

      <h3 className="section">Runs</h3>
      <ul className="runs">
        {issue.runs.map((runItem) => (
          <li key={runItem.id}>
            <span className="role">{ROLE_LABEL[runItem.role] ?? runItem.role}</span>
            <StatusPill status={runItem.status} />
            <span className="dim">{runItem.adapter}</span>
            {runItem.flags.length > 0 && <span className="flag">{runItem.flags.join(", ").replaceAll("_", " ").toLowerCase()}</span>}
            {runItem.error && <span className="error inline">{runItem.error}</span>}
          </li>
        ))}
      </ul>

      <h3 className="section">Replays</h3>
      <ul className="runs">
        {issue.replays.map((replay) => (
          <li key={replay.id}>
            <span className="role">{replay.kind}</span>
            <StatusPill status={replay.status} />
            {replay.failedTests?.length ? <span className="dim">failing: {replay.failedTests.join(", ")}</span> : null}
          </li>
        ))}
      </ul>

      <h3 className="section">Conversation</h3>
      <ul className="comments">
        {issue.comments.map((comment) => (
          <li key={comment.id} className={comment.pinned ? "pinned" : ""}>
            <span className="who">{comment.authorType}</span>
            <p>{comment.body}</p>
          </li>
        ))}
        {issue.comments.length === 0 && <li className="dim">nothing said yet</li>}
      </ul>
    </section>
  );
}

function RunSummary({
  issue,
  run,
  showDiff,
  onToggleDiff,
}: {
  issue: Issue;
  run: { id: string; status: string; flags: string[]; error: string | null };
  showDiff: boolean;
  onToggleDiff: () => void;
}) {
  const [log, setLog] = useState<string | null>(null);
  const [diff, setDiff] = useState<string | null>(null);

  async function load() {
    const data = await gql<{ run: { log: string; diff: string } }>(
      `query ($id: UUID!) { run(id: $id) { log diff } }`,
      { id: run.id },
    );
    setLog(data.run.log);
    setDiff(data.run.diff);
  }

  return (
    <div className="running">
      {run.error && <p className="error">{run.error}</p>}
      <div className="row">
        <button className="ghost" onClick={() => void load()}>
          {log ? "Reload log" : "Show log"}
        </button>
        <button className="ghost" onClick={onToggleDiff}>
          {showDiff ? "Hide diff" : "Show diff"}
        </button>
      </div>
      {log && <pre className="log">{log.split("\n").slice(-120).join("\n")}</pre>}
      {showDiff && diff && <pre className="log">{diff}</pre>}
      {showDiff && !diff && <p className="dim">nothing captured yet for {issue.title}</p>}
    </div>
  );
}
