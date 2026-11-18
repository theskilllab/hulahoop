import { useState } from "react";
import { gql } from "../api";

interface Props {
  projectId: string;
  /** The repo the issue will be tested against; null when the project has none. */
  repoId: string | null;
  onAddRepo: (path: string) => Promise<void>;
  onCreated: (id: string) => Promise<void> | void;
  onCancel: () => void;
}

export default function NewIssue({ projectId, repoId, onAddRepo, onCreated, onCancel }: Props) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [criteria, setCriteria] = useState("");
  const [repoPath, setRepoPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function attachRepo() {
    setBusy(true);
    setError(null);
    try {
      // The parent reloads the workspace, so the repo arrives as a prop and this
      // form turns into the issue form on the next render.
      await onAddRepo(repoPath.trim());
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
      const data = await gql<{ createIssue: { id: string } }>(
        `mutation ($projectId: UUID!, $repoId: UUID!, $title: String!, $body: String!, $criteria: [String!]!) {
           createIssue(projectId: $projectId, repoId: $repoId, title: $title, body: $body, criteria: $criteria) { id }
         }`,
        { projectId, repoId, title: title.trim(), body: body.trim(), criteria: lines },
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
        Title
        <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What must be true when this is done?" />
      </label>
      <label>
        Context
        <textarea value={body} onChange={(e) => setBody(e.target.value)} rows={4} placeholder="Anything the agents should know." />
      </label>
      <label>
        Acceptance criteria — one per line
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
        <button className="primary" disabled={busy || !title.trim() || !criteria.trim()} onClick={() => void create()}>
          Create issue
        </button>
      </div>
    </section>
  );
}
