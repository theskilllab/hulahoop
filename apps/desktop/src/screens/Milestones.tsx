import { useState } from "react";
import { gql, type Artifact, type Issue, type Milestone, type Project } from "../api";

interface Props {
  workspaceId: string;
  projects: Project[];
  milestones: Milestone[];
  issues: Issue[];
  artifacts: Artifact[];
  onChanged: () => Promise<void> | void;
}

/**
 * Milestones are bodies of work with a name, a date and a shelf. Nothing enforces
 * that an issue contributes to one — the milestone is a label for humans, and the
 * loop keeps its own state machine on the issue.
 */
export default function Milestones({ workspaceId, projects, milestones, issues, artifacts, onChanged }: Props) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [target, setTarget] = useState("");
  const [projectId, setProjectId] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [linking, setLinking] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  async function create() {
    if (title.trim().length < 3) {
      setError("a milestone needs a title, so it can be referred to later");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await gql(
        `mutation ($workspaceId: UUID!, $projectId: UUID, $title: String!, $body: String, $targetDate: String) {
          createMilestone(workspaceId: $workspaceId, projectId: $projectId, title: $title, body: $body, targetDate: $targetDate) { id }
        }`,
        {
          workspaceId,
          projectId: projectId || null,
          title: title.trim(),
          body,
          targetDate: target || null,
        },
      );
      setTitle("");
      setBody("");
      setTarget("");
      await onChanged();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  async function close(id: string, status: string) {
    await gql(`mutation ($id: UUID!, $status: String!) { updateMilestone(id: $id, status: $status) { id status } }`, {
      id,
      status,
    });
    await onChanged();
  }

  async function link(issueId: string, milestoneId: string | null) {
    await gql(
      `mutation ($issueId: UUID!, $milestoneId: UUID) { setIssueMilestone(issueId: $issueId, milestoneId: $milestoneId) { id milestoneId } }`,
      { issueId, milestoneId },
    );
    setLinking(null);
    await onChanged();
  }

  return (
    <section className="screen page">
      <header className="sheet-head">
        <div>
          <h1>Milestones</h1>
          <p className="hint">
            A name for a body of work. Issues point at one; nothing about the loop depends on it.
          </p>
        </div>
        <button className="primary" onClick={() => setCreating(!creating)} aria-expanded={creating}>
          {creating ? "Cancel" : "New milestone"}
        </button>
      </header>

      {creating ? (
      <div className="panel">
        <h2>New milestone</h2>
        <div className="grid-2">
          <label>
            <span>title</span>
            <input value={title} onChange={(event) => setTitle(event.target.value)} placeholder="GitHub identity" />
          </label>
          <label>
            <span>project (optional)</span>
            <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
              <option value="">workspace</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.prefix ? `${project.prefix} · ${project.name}` : project.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>target date (optional)</span>
            <input value={target} onChange={(event) => setTarget(event.target.value)} placeholder="2026-12-01" />
          </label>
          <label className="wide">
            <span>what it is for</span>
            <textarea value={body} onChange={(event) => setBody(event.target.value)} rows={3} />
          </label>
        </div>
        {error && <p className="error">{error}</p>}
        <div className="row">
          <button
            className="primary"
            onClick={() => void create().then(() => setCreating(false))}
            disabled={busy}
          >
            {busy ? "creating…" : "create milestone"}
          </button>
          <button className="ghost" onClick={() => setCreating(false)}>
            Cancel
          </button>
        </div>
      </div>
      ) : null}

      {milestones.length === 0 && <p className="hint">No milestones yet.</p>}

      {milestones.map((milestone) => {
        const members = issues.filter((issue) => issue.milestoneId === milestone.id);
        const files = artifacts.filter((artifact) => artifact.milestoneId === milestone.id);
        const project = projects.find((entry) => entry.id === milestone.projectId);
        return (
          <article key={milestone.id} className={`panel milestone ${milestone.status}`}>
            <header>
              <h2>
                {milestone.title}
                <span className={`pill ${milestone.status}`}>{milestone.status}</span>
              </h2>
              <p className="dim">
                {project ? `${project.prefix ?? "—"} · ${project.name}` : "workspace"}
                {milestone.targetDate ? ` · target ${milestone.targetDate}` : ""} · {members.length} issue
                {members.length === 1 ? "" : "s"} · {files.length} file{files.length === 1 ? "" : "s"}
              </p>
              {milestone.body && <p>{milestone.body}</p>}
              <button className="link" onClick={() => void close(milestone.id, milestone.status === "open" ? "closed" : "open")}>
                {milestone.status === "open" ? "close milestone" : "reopen milestone"}
              </button>
            </header>

            <ul className="members">
              {members.map((issue) => (
                <li key={issue.id}>
                  <code>{issue.key}</code> {issue.title} <span className={`pill ${issue.status}`}>{issue.status}</span>
                </li>
              ))}
              {members.length === 0 && <li className="dim">nothing filed here yet</li>}
            </ul>

            <div className="attach">
              {linking === milestone.id ? (
                <select
                  autoFocus
                  defaultValue=""
                  onChange={(event) => {
                    const value = event.target.value;
                    if (value) void link(value, milestone.id);
                  }}
                  onBlur={() => setLinking(null)}
                >
                  <option value="">pick an issue…</option>
                  {issues
                    .filter((issue) => issue.milestoneId !== milestone.id)
                    .map((issue) => (
                      <option key={issue.id} value={issue.id}>
                        {issue.key} — {issue.title}
                      </option>
                    ))}
                </select>
              ) : (
                <button className="link" onClick={() => setLinking(milestone.id)}>
                  + link an issue
                </button>
              )}
              {files.map((file) => (
                <span key={file.id} className="chip">
                  {file.filename}
                </span>
              ))}
            </div>
          </article>
        );
      })}
    </section>
  );
}
