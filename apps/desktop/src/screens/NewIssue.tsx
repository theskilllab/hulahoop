import { useState } from "react";
import { gql, type Project } from "../api";

interface Props {
  workspaceId: string;
  projects: Project[];
  onAddRepo: (path: string, projectId: string | null) => Promise<void>;
  onCreated: (id: string) => Promise<void> | void;
  onCancel: () => void;
}

export default function NewIssue({ workspaceId, projects, onAddRepo, onCreated, onCancel }: Props) {
  const first = projects[0] ?? null;
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [criteria, setCriteria] = useState("");
  const [projectId, setProjectId] = useState<string>(first?.id ?? "");
  const [repoPath, setRepoPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const project = projects.find((entry) => entry.id === projectId) ?? null;
  // An issue may be filed against no project at all, in which case any repository
  // in the workspace will do — but a run still needs one, so ask before it starts.
  const repoId = project?.repos[0]?.id ?? (projectId ? null : first?.repos[0]?.id ?? null);

  // A disabled button that will not say why is a dead end; name what is missing.
  const missing = [title.trim() ? null : "a title", criteria.trim() ? null : "a criterion", repoId ? null : "a repository"]
    .filter(Boolean)
    .join(" and ");

  async function attachRepo() {
    setBusy(true);
    setError(null);
    try {
      await onAddRepo(repoPath.trim(), projectId || null);
      setRepoPath("");
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  async function create() {
    setBusy(true);
    setError(null);
    try {
      const lines = criteria
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean);
      // The server is the authority; this only turns a round trip into a sentence
      // next to the field, with the same rules and the same wording.
      if (!title.trim()) throw new Error("the issue needs a title");
      if (lines.length === 0) throw new Error("an issue needs at least one acceptance criterion, one per line");
      for (const [index, line] of lines.entries()) {
        if (line.length < 8) {
          throw new Error(
            `criterion ${index + 1} is too short to test: "${line}". Write the observable behavior and the condition, for example add(2,3) returns 5.`,
          );
        }
      }
      const data = await gql<{ createIssue: { id: string } }>(
        `mutation ($workspaceId: UUID!, $projectId: UUID, $repoId: UUID, $title: String!, $body: String!, $criteria: [String!]!) {
           createIssue(workspaceId: $workspaceId, projectId: $projectId, repoId: $repoId, title: $title, body: $body, criteria: $criteria) { id }
         }`,
        {
          workspaceId,
          projectId: projectId || null,
          repoId,
          title: title.trim(),
          body: body.trim(),
          criteria: lines,
        },
      );
      await onCreated(data.createIssue.id);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  if (!repoId) {
    return (
      <section className="detail narrow">
        <h1>Add a repository</h1>
        <p className="dim">
          Point Hulahoop at a checkout of a repository whose <code>flake.nix</code> and <code>justfile</code> already work. The
          contract is checked before the repo is stored.
        </p>
        {error && <p className="error">{error}</p>}
        <label>
          Project
          <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
            <option value="">no project — {workspaceId.slice(0, 8)}</option>
            {projects.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
              </option>
            ))}
          </select>
        </label>
        <input placeholder="/absolute/path/to/repo" value={repoPath} onChange={(e) => setRepoPath(e.target.value)} />
        <div className="row">
          <button className="ghost" onClick={onCancel}>
            Cancel
          </button>
          <button className="primary" disabled={busy || repoPath.trim().length < 2} onClick={() => void attachRepo()}>
            Add repository
          </button>
        </div>
      </section>
    );
  }

  return (
    <section className="detail narrow">
      <h1>New issue</h1>
      {error && <p className="error">{error}</p>}
      <label>
        Project
        <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
          <option value="">no project</option>
          {projects.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
            </option>
          ))}
        </select>
      </label>
      <p className="dim small">
        The key comes from the project: {project?.prefix ? `${project.prefix}-1` : "no project, so the workspace prefix"}. A run
        needs a project, because the agent roles are bound per project.
      </p>
      <label>
        Title <span className="required">required</span>
        <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What must be true when this is done?" />
      </label>
      <label>
        Context
        <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={4} placeholder="Anything the agents should know." />
      </label>
      <label>
        Acceptance criteria — one per line <span className="required">required</span>
        <textarea
          value={criteria}
          onChange={(e) => setCriteria(e.target.value)}
          rows={6}
          placeholder={"returns 401 when the token is expired\nrejects a request without a body"}
        />
      </label>
      <p className="dim small">
        Each criterion becomes exactly one test. A criterion that names internals or could be true of any program is refused at
        freeze time.
      </p>
      <div className="row">
        <button className="ghost" onClick={onCancel}>
          Cancel
        </button>
        <button className="primary" disabled={busy} onClick={() => void create()}>
          {busy ? "creating…" : "Create issue"}
        </button>
        {missing && <span className="dim small">needs {missing}</span>}
      </div>
    </section>
  );
}
