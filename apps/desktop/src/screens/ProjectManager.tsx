import { useEffect, useRef, useState } from "react";
import {
  attachGithubRepo,
  gql,
  loadGithubRepos,
  renameProject,
  type GithubRepo,
  type Project,
  type Workspace,
} from "../api";

/** The project and repository controls live beside their project cards. */
export default function ProjectManager({ workspace, project, onChanged, onClose }: {
  workspace: Workspace;
  project: Project | null;
  onChanged: () => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(project?.name ?? "");
  const [prefix, setPrefix] = useState(project?.prefix ?? "");
  const [repoPath, setRepoPath] = useState("");
  const [githubRepos, setGithubRepos] = useState<GithubRepo[]>([]);
  const [githubChoice, setGithubChoice] = useState("");
  const [githubError, setGithubError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [base, setBase] = useState(project?.repos[0]?.baseBookmark ?? "main");
  const nameInput = useRef<HTMLInputElement>(null);

  useEffect(() => { setName(project?.name ?? ""); }, [project?.name]);

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    nameInput.current?.focus();
    return () => { if (opener?.isConnected) opener.focus(); };
  }, []);

  useEffect(() => {
    let live = true;
    loadGithubRepos().then((repos) => { if (live) setGithubRepos(repos); })
      .catch((cause: Error) => { if (live) setGithubError(cause.message); });
    return () => { live = false; };
  }, []);

  async function action(work: () => Promise<unknown>, close = false) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await onChanged();
      if (close) onClose();
    } catch (cause) {
      // Multiple edits can partially succeed; refresh the cards and show that
      // outcome before the user decides whether to retry the remaining edit.
      await onChanged().catch(() => undefined);
      setError(`Some changes may have saved. ${String(cause instanceof Error ? cause.message : cause)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="settings-block project-manager" aria-label={project ? `${project.name} settings` : "New project"}>
      <div className="row spread">
        <h2>{project ? `Project · ${project.name}` : "New project"}</h2>
        <button className="ghost" onClick={onClose}>Close</button>
      </div>
      <p className="dim small">A project owns its task prefix and repositories. The first repository is used for runs.</p>
      <div className="grid-2">
        <label><span>Project name</span><input type="text" ref={nameInput} value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label><span>Task prefix</span><input type="text" maxLength={5} value={prefix}
          onChange={(event) => setPrefix(event.target.value.toUpperCase())} /></label>
      </div>
      {project ? (
        <button disabled={busy || name.trim().length < 2 || prefix.trim().length < 2}
          onClick={() => void action(async () => {
            if (name.trim() !== project.name) await renameProject(project.id, name.trim());
            if (prefix.trim() !== project.prefix) await gql(
              `mutation ($id: UUID!, $prefix: String!) { setProjectPrefix(projectId: $id, prefix: $prefix) { id prefix } }`,
              { id: project.id, prefix: prefix.trim() },
            );
          })}>Save project</button>
      ) : (
        <button className="primary" disabled={busy || name.trim().length < 2}
          onClick={() => void action(() => gql(
            `mutation ($workspaceId: UUID!, $name: String!, $prefix: String) { createProject(workspaceId: $workspaceId, name: $name, prefix: $prefix) { id } }`,
            { workspaceId: workspace.id, name: name.trim(), prefix: prefix.trim() || null },
          ), true)}>Create project</button>
      )}
      {project && (
        <>
          <h3>Repositories</h3>
          {project.repos.length ? <ul className="repos">
            {project.repos.map((repo) => <li key={repo.id}>
              <strong>{repo.name}</strong> <span className="dim small">{repo.path ?? "mirror"}</span>
              {repo.remoteUrl && <div className="dim small">{repo.remoteUrl}</div>}
              <div className="dim small">{repo.vcs} · base {repo.baseBookmark} · fetched {repo.fetchedAt
                ? new Date(repo.fetchedAt).toLocaleString() : "never"}</div>
            </li>)}
          </ul> : <p className="dim">No repository attached yet.</p>}
          {githubRepos.length > 0 && <div className="row">
            <select aria-label="GitHub repository" value={githubChoice}
              onChange={(event) => setGithubChoice(event.target.value)}>
              <option value="">Pick a repository the app can reach…</option>
              {githubRepos.filter((entry) => !project.repos.some((repo) => repo.remoteUrl === entry.cloneUrl))
                .map((entry) => <option key={entry.cloneUrl} value={entry.cloneUrl}>
                  {entry.fullName}{entry.private ? " (private)" : ""}{entry.archived ? " (archived)" : ""}
                </option>)}
            </select>
            <button disabled={busy || !githubChoice} onClick={() => void action(async () => {
              const selected = githubRepos.find((entry) => entry.cloneUrl === githubChoice);
              if (selected) await attachGithubRepo(project.id, selected);
              setGithubChoice("");
            })}>Attach from GitHub</button>
          </div>}
          {githubError && <p className="dim small">{githubError}</p>}
          <div className="row">
            <input type="text" aria-label="Local repository path" placeholder="/absolute/path/to/repo"
              value={repoPath} onChange={(event) => setRepoPath(event.target.value)} />
            <button disabled={busy || repoPath.trim().length < 2} onClick={() => void action(async () => {
              await gql(
                `mutation ($projectId: UUID!, $name: String!, $path: String!) { addRepo(projectId: $projectId, name: $name, path: $path) { id } }`,
                { projectId: project.id, name: repoPath.trim().split("/").filter(Boolean).pop() ?? "repo", path: repoPath.trim() },
              );
              setRepoPath("");
            })}>Add repository</button>
          </div>
          {project.repos[0] && <div className="row">
            <label><span>Base bookmark · {project.repos[0].name}</span>
              <input type="text" value={base} onChange={(event) => setBase(event.target.value)} /></label>
            <button disabled={busy || !base.trim()} onClick={() => void action(() => gql(
              `mutation ($id: UUID!, $bookmark: String!) { updateRepo(id: $id, baseBookmark: $bookmark) { id } }`,
              { id: project.repos[0].id, bookmark: base.trim() },
            ))}>Save base</button>
            <button disabled={busy} onClick={() => void action(() => gql(
              `mutation ($id: UUID!) { fetchRepo(id: $id) { id } }`, { id: project.repos[0].id },
            ))}>Fetch base</button>
          </div>}
        </>
      )}
      {error && <p className="error" role="alert">{error}</p>}
    </section>
  );
}
