import { useEffect, useState } from "react";
import PageHeader from "../PageHeader";
import {
  attachGithubRepo,
  createWorkspace,
  gql,
  loadGithubRepos,
  loadWorkspaceMembers,
  renameProject,
  updateWorkspace,
  type Agent,
  type GithubRepo,
  type Project,
  type Repo,
  type Skill,
  type Workspace,
  type WorkspaceMember,
} from "../api";

export default function Settings({
  workspace,
  workspaces,
  projects,
  active,
  agents,
  onChanged,
  onSwitchWorkspace,
}: {
  workspace: Workspace;
  workspaces: Workspace[];
  projects: Project[];
  active: Project | null;
  agents: Agent[];
  skills?: Skill[];
  onChanged: () => Promise<void>;
  onSwitchWorkspace: (id: string) => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [chosen, setChosen] = useState<string>(active?.id ?? "");
  const [prefix, setPrefix] = useState<string>(active?.prefix ?? "");
  const [newProject, setNewProject] = useState("");
  const [newPrefix, setNewPrefix] = useState("");
  const [repoPath, setRepoPath] = useState("");
  const [githubRepos, setGithubRepos] = useState<GithubRepo[]>([]);
  const [githubChoice, setGithubChoice] = useState("");
  const [githubError, setGithubError] = useState<string | null>(null);
  const [wsName, setWsName] = useState(workspace.name);
  const [wsPrefix, setWsPrefix] = useState(workspace.defaultPrefix);
  const [members, setMembers] = useState<WorkspaceMember[]>([]);
  const [newWorkspace, setNewWorkspace] = useState("");
  const [newWorkspacePrefix, setNewWorkspacePrefix] = useState("");
  const [projectName, setProjectName] = useState("");

  const project = projects.find((entry) => entry.id === chosen) ?? active ?? projects[0] ?? null;
  const repo: Repo | undefined = project?.repos[0];

  useEffect(() => {
    loadGithubRepos()
      .then(setGithubRepos)
      .catch((cause: Error) => {
        setGithubRepos([]);
        setGithubError(cause.message);
      });
  }, []);

  useEffect(() => {
    setWsName(workspace.name);
    setWsPrefix(workspace.defaultPrefix);
    void loadWorkspaceMembers(workspace.id).then(setMembers).catch(() => setMembers([]));
  }, [workspace.id, workspace.name, workspace.defaultPrefix]);

  useEffect(() => {
    setProjectName(project?.name ?? "");
    setPrefix(project?.prefix ?? "");
  }, [project?.id, project?.name, project?.prefix]);

  async function saveWorkspace(name: string, prefix: string) {
    setBusy("workspace");
    setError(null);
    try {
      await updateWorkspace(workspace.id, name.trim(), prefix.trim());
      await onChanged();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(null);
    }
  }

  async function addWorkspace() {
    setBusy("new-workspace");
    setError(null);
    try {
      const created = await createWorkspace(
        newWorkspace.trim(),
        newWorkspacePrefix.trim() || undefined,
      );
      setNewWorkspace("");
      setNewWorkspacePrefix("");
      await onSwitchWorkspace(created.id);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(null);
    }
  }

  async function saveProjectName() {
    if (!project) return;
    setBusy("project-name");
    setError(null);
    try {
      await renameProject(project.id, projectName.trim());
      await onChanged();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(null);
    }
  }

  /** A new harness or model invalidates its probe, so ask for a fresh one. */
  async function action(label: string, work: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    try {
      await work();
      await onChanged();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="detail">
      <PageHeader
        subtitle={`${workspaces.length === 1 ? "one workspace" : `${workspaces.length} workspaces`} · ${agents.length} agents`}
        title="Settings"
      />

      <div className="settings-block">
        <div className="row spread">
          <strong>{workspace.name}</strong>
          <span className="dim small">{workspace.slug} · workspace</span>
        </div>
        <p className="dim small">
          Issues filed with no project are keyed by this prefix — {workspace.defaultPrefix}-1, {workspace.defaultPrefix}-2.
          Projects in this workspace must use a different one.
        </p>
        <div className="grid-2">
          <label>
            <span>name</span>
            <input value={wsName} onChange={(event) => setWsName(event.target.value)} />
          </label>
          <label>
            <span>issue prefix (2–5)</span>
            <input
              value={wsPrefix}
              maxLength={5}
              onChange={(event) => setWsPrefix(event.target.value.toUpperCase())}
            />
          </label>
        </div>
        <div className="row">
          <button
            className="primary"
            disabled={busy === "workspace"}
            onClick={() => void saveWorkspace(wsName, wsPrefix)}
          >
            Save workspace
          </button>
          <span className="dim small">
            {members.length === 0
              ? "one member"
              : members.map((member) => `${member.name} (${member.role})`).join(", ")}
          </span>
        </div>
        <div className="row">
          <input
            placeholder="new workspace name"
            value={newWorkspace}
            onChange={(event) => setNewWorkspace(event.target.value)}
          />
          <input
            placeholder="prefix"
            value={newWorkspacePrefix}
            maxLength={5}
            onChange={(event) => setNewWorkspacePrefix(event.target.value.toUpperCase())}
          />
          <button
            className="ghost"
            disabled={busy === "new-workspace" || newWorkspace.trim().length < 2}
            onClick={() => void addWorkspace()}
          >
            Add workspace
          </button>
        </div>
        {workspaces.length > 1 && (
          <p className="dim small">
            {workspaces.length} workspaces on this server:{" "}
            {workspaces.map((entry) => entry.name).join(", ")}.
          </p>
        )}
      </div>
      {error && <p className="error">{error}</p>}

      <h2 className="section">Projects</h2>
      <p className="dim small">
        A project carries the prefix its issues are named by — {workspace.defaultPrefix} is the workspace's own, for issues
        with no project. Several repositories may hang off one project; the first is the one a run clones.
      </p>
      <div className="settings-block">
        <div className="row">
          <select value={chosen} onChange={(event) => setChosen(event.target.value)}>
            {projects.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
              </option>
            ))}
          </select>
          {project && (
            <>
              <input
                className="narrow-input"
                value={projectName}
                placeholder={project.name}
                onChange={(event) => setProjectName(event.target.value)}
                onBlur={() => {
                  if (projectName.trim() && projectName.trim() !== project.name) {
                    void saveProjectName();
                  }
                }}
              />
              <input
                className="narrow-input"
                value={prefix}
                placeholder={project.prefix ?? "PREFIX"}
                onChange={(event) => setPrefix(event.target.value)}
              />
              <button
                disabled={busy !== null || prefix.trim().length < 2}
                onClick={() =>
                  action("prefix", () =>
                    gql(`mutation ($id: UUID!, $prefix: String!) { setProjectPrefix(projectId: $id, prefix: $prefix) { id prefix } }`, {
                      id: project.id,
                      prefix: prefix.trim(),
                    }),
                  )
                }
              >
                Set prefix
              </button>
            </>
          )}
        </div>
        <div className="row">
          <input value={newProject} onChange={(event) => setNewProject(event.target.value)} placeholder="new project name" />
          <input
            className="narrow-input"
            value={newPrefix}
            onChange={(event) => setNewPrefix(event.target.value)}
            placeholder="BAL"
          />
          <button
            disabled={busy !== null || newProject.trim().length < 2}
            onClick={() =>
              action("project", async () => {
                await gql(
                  `mutation ($workspaceId: UUID!, $name: String!, $prefix: String) { createProject(workspaceId: $workspaceId, name: $name, prefix: $prefix) { id } }`,
                  {
                    workspaceId: workspace.id,
                    name: newProject.trim(),
                    prefix: newPrefix.trim() || null,
                  },
                );
                setNewProject("");
                setNewPrefix("");
              })
            }
          >
            Create project
          </button>
        </div>
      </div>

      <h2 className="section">Repositories</h2>
      {project ? (
        <div className="settings-block">
          <ul className="repos">
            {project.repos.map((entry) => (
              <li key={entry.id}>
                <strong>{entry.name}</strong>{" "}
                <span className="dim">{entry.path ?? "mirror, cloned on the first run"}</span>
                {entry.remoteUrl && <div className="dim small">{entry.remoteUrl}</div>}
                <div className="dim small">
                  {entry.vcs} · contract v{entry.contractVersion} · base {entry.baseBookmark} · fetched{" "}
                  {entry.fetchedAt ? new Date(entry.fetchedAt).toLocaleString() : "never"}
                </div>
              </li>
            ))}
            {project.repos.length === 0 && <li className="dim">No repository attached yet.</li>}
          </ul>
          {githubRepos.length > 0 && (
            <div className="row">
              <select
                value={githubChoice}
                onChange={(event) => setGithubChoice(event.target.value)}
              >
                <option value="">pick a repository the app can reach…</option>
                {githubRepos
                  .filter(
                    (entry) =>
                      !project.repos.some((attached) => attached.remoteUrl === entry.cloneUrl),
                  )
                  .map((entry) => (
                    <option key={entry.cloneUrl} value={entry.cloneUrl}>
                      {entry.fullName}
                      {entry.private ? " (private)" : ""}
                      {entry.archived ? " (archived)" : ""}
                    </option>
                  ))}
              </select>
              <button
                disabled={busy !== null || githubChoice === ""}
                onClick={() =>
                  action("github-repo", async () => {
                    const chosen = githubRepos.find((entry) => entry.cloneUrl === githubChoice);
                    if (!chosen) {
                      return;
                    }
                    await attachGithubRepo(project.id, chosen);
                    setGithubChoice("");
                  })
                }
              >
                Attach from GitHub
              </button>
            </div>
          )}
          {githubError && <p className="dim small">{githubError}</p>}
          <div className="row">
            <input
              value={repoPath}
              onChange={(event) => setRepoPath(event.target.value)}
              placeholder="/absolute/path/to/another/repo"
            />
            <button
              disabled={busy !== null || repoPath.trim().length < 2}
              onClick={() =>
                action("repo", async () => {
                  await gql(
                    `mutation ($projectId: UUID!, $name: String!, $path: String!) { addRepo(projectId: $projectId, name: $name, path: $path) { id } }`,
                    {
                      projectId: project.id,
                      name: repoPath.trim().split("/").filter(Boolean).pop() ?? "repo",
                      path: repoPath.trim(),
                    },
                  );
                  setRepoPath("");
                })
              }
            >
              Add repository
            </button>
          </div>
        </div>
      ) : (
        <p className="dim">Create a project first.</p>
      )}

      <h2 className="section">Repository</h2>
      {repo ? (
        <div className="settings-block">
          <p>
            <strong>{repo.name}</strong> <span className="dim">{repo.path}</span>
          </p>
          <p className="dim small">
            {repo.vcs} · contract v{repo.contractVersion} · base {repo.baseBookmark} · fetched{" "}
            {repo.fetchedAt ? new Date(repo.fetchedAt).toLocaleString() : "never"}
          </p>
          <div className="row">
            <button
              disabled={busy !== null}
              onClick={() =>
                action("fetch", () =>
                  gql(`mutation ($id: UUID!) { fetchRepo(id: $id) { id } }`, { id: repo.id }),
                )
              }
            >
              Fetch base
            </button>
            <button
              disabled={busy !== null}
              onClick={() =>
                action("bearings", () =>
                  gql(`mutation ($id: UUID!, $bookmark: String!) { updateRepo(id: $id, baseBookmark: $bookmark) { id } }`, {
                    id: repo.id,
                    bookmark: "main",
                  }),
                )
              }
            >
              Set base to main
            </button>
          </div>
        </div>
      ) : (
        <p className="dim">No repository attached yet.</p>
      )}

      <h2 className="section">Connectors</h2>
      <p className="dim small">
        GitHub, the Claude sign-ins and the Pi login live on the <strong>Connectors</strong> screen,
        so there is one place that answers "what can the loop act as?".
      </p>

      <h2 className="section">Agents</h2>
      <p className="dim small">
        Agents are created and edited on the <strong>Agents</strong> screen: harness, model, its own
        AGENTS.md and the skills it carries. Which of them a status calls is the workflow's
        decision, written down once in WORKFLOWS.md.
      </p>
    </section>
  );
}
