import { useCallback, useEffect, useMemo, useState } from "react";
import {
  connect,
  gql,
  loadArtifacts,
  loadMilestones,
  loadSkills,
  loadWorkflow,
  loadWorkspace,
  loadWorkspaceIssues,
  savedRemote,
  useLocalServer,
  type Agent,
  type Artifact,
  type Connection,
  type Issue,
  type Milestone,
  type Project,
  type Workspace,
  type Skill,
  type WorkflowDoc,
} from "./api";
import { WORKSPACE_KEY } from "./api";
import { applyTheme, savedTheme, type Theme } from "./theme";
import Artifacts from "./screens/Artifacts";
import Board from "./screens/Board";
import Connect from "./screens/Connect";
import IssueDetail from "./screens/IssueDetail";
import Milestones from "./screens/Milestones";
import Skills from "./screens/Skills";
import WorkflowScreen from "./screens/Workflow";
import NewIssue from "./screens/NewIssue";
import Settings from "./screens/Settings";

type View =
  | { name: "board" }
  | { name: "issue"; id: string }
  | { name: "new" }
  | { name: "workflow" }
  | { name: "skills" }
  | { name: "milestones" }
  | { name: "artifacts" }
  | { name: "settings" }
  | { name: "connect" };

/** The mark: a filled circle in the brand indigo, the same everywhere. */
function Logo() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r="7" fill="var(--brand)" />
    </svg>
  );
}

export default function App() {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [issues, setIssues] = useState<Issue[]>([]);
  const [milestones, setMilestones] = useState<Milestone[]>([]);
  const [workflow, setWorkflow] = useState<WorkflowDoc | null>(null);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [view, setView] = useState<View>({ name: "board" });
  const [connected, setConnected] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  /** `all` shows every project's issues, including the ones filed under none. */
  const [projectFilter, setProjectFilter] = useState<string | "all">("all");
  const [theme, setTheme] = useState<Theme>(savedTheme());

  const refresh = useCallback(async () => {
    if (!workspace) return;
    try {
      const [nextIssues, nextMilestones, nextArtifacts, nextWorkflow, nextSkills] = await Promise.all([
        loadWorkspaceIssues(workspace.id),
        loadMilestones(workspace.id),
        loadArtifacts(workspace.id),
        loadWorkflow(workspace.id),
        loadSkills(workspace.id),
      ]);
      setIssues(nextIssues);
      setMilestones(nextMilestones);
      setArtifacts(nextArtifacts);
      setWorkflow(nextWorkflow);
      setSkills(nextSkills);
      setError(null);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    }
  }, [workspace]);

  const load = useCallback(async (preferred?: string | null) => {
    const info = await connect();
    setConnection(info);
    const remembered = preferred ?? localStorage.getItem(WORKSPACE_KEY);
    const loaded = await loadWorkspace(remembered);
    setWorkspaces(loaded.workspaces);
    setWorkspace(loaded.workspace);
    localStorage.setItem(WORKSPACE_KEY, loaded.workspace.id);
    setProjects(loaded.projects);
    setAgents(loaded.agents);
    setError(null);
  }, []);

  /** A different workspace means different projects, issues and shelves. */
  const switchWorkspace = useCallback(
    async (id: string) => {
      localStorage.setItem(WORKSPACE_KEY, id);
      await load(id);
      setView({ name: "board" });
    },
    [load],
  );

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        if (cancelled) return;
        await load();
        setConnected(true);
      } catch (cause) {
        setError(String(cause instanceof Error ? cause.message : cause));
        setView({ name: "connect" });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [load]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 2500);
    return () => clearInterval(timer);
  }, [refresh]);

  const reload = useCallback(async () => {
    const loaded = await loadWorkspace();
    setWorkspace(loaded.workspace);
    setProjects(loaded.projects);
    setAgents(loaded.agents);
  }, []);

  const shown = useMemo(
    () => (projectFilter === "all" ? issues : issues.filter((issue) => issue.projectId === projectFilter)),
    [issues, projectFilter],
  );
  const openIssue = issues.find((issue) => view.name === "issue" && issue.id === view.id) ?? null;
  const counts = useMemo(() => {
    const map = new Map<string, number>();
    for (const issue of shown) map.set(issue.status, (map.get(issue.status) ?? 0) + 1);
    return map;
  }, [shown]);
  const project = useMemo(
    () => projects.find((entry) => entry.id === projectFilter) ?? projects[0] ?? null,
    [projects, projectFilter],
  );

  async function createRepo(path: string, projectId: string | null) {
    const target = projects.find((entry) => entry.id === projectId) ?? project;
    if (!target) return;
    await gql(
      `mutation ($projectId: UUID!, $name: String!, $path: String!) { addRepo(projectId: $projectId, name: $name, path: $path) { id name } }`,
      { projectId: target.id, name: path.split("/").filter(Boolean).pop() ?? "repo", path },
    );
    await reload();
    setNotice("Repository added. Preflight the agents from Settings before starting work.");
  }

  if (view.name === "connect" || !connected) {
    return (
      <Connect
        failure={error}
        onCancel={connected ? () => setView({ name: "board" }) : undefined}
        onConnected={() => {
          setConnected(false);
          setView({ name: "board" });
          void load()
            .then(() => setConnected(true))
            .catch((cause) => {
              setError(String(cause instanceof Error ? cause.message : cause));
              setView({ name: "connect" });
            });
        }}
      />
    );
  }

  if (error && !workspace) {
    return (
      <main className="boot">
        <h1>hulahoop</h1>
        <p className="error">{error}</p>
        <button onClick={() => setView({ name: "connect" })}>connect to another server</button>
      </main>
    );
  }

  if (!workspace) {
    return (
      <main className="boot">
        <h1>hulahoop</h1>
        <p className="hint">loading the workspace…</p>
      </main>
    );
  }

  return (
    <div className="shell">
      <aside className="rail">
        <div className="brand">
          <Logo />
          hulahoop
        </div>
        <label className="project-pick">
          <span>workspace</span>
          <select value={workspace.id} onChange={(event) => void switchWorkspace(event.target.value)}>
            {workspaces.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name} · {entry.defaultPrefix}
              </option>
            ))}
          </select>
        </label>
        <label className="project-pick">
          <span>project</span>
          <select value={projectFilter} onChange={(event) => setProjectFilter(event.target.value)}>
            <option value="all">all projects</option>
            {projects.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
              </option>
            ))}
          </select>
        </label>
        <nav>
          <button className={view.name === "board" ? "on" : ""} onClick={() => setView({ name: "board" })}>
            Board <span className="tally">{shown.filter((i) => i.status !== "done" && i.status !== "cancelled").length}</span>
          </button>
          <button className={view.name === "new" ? "on" : ""} onClick={() => setView({ name: "new" })}>
            New issue
          </button>
          <button className={view.name === "workflow" ? "on" : ""} onClick={() => setView({ name: "workflow" })}>
            Workflow <span className="tally">{workflow?.sections.length ?? 0}</span>
          </button>
          <button className={view.name === "skills" ? "on" : ""} onClick={() => setView({ name: "skills" })}>
            Skills <span className="tally">{skills.length}</span>
          </button>
          <button className={view.name === "milestones" ? "on" : ""} onClick={() => setView({ name: "milestones" })}>
            Milestones <span className="tally">{milestones.filter((m) => m.status === "open").length}</span>
          </button>
          <button className={view.name === "artifacts" ? "on" : ""} onClick={() => setView({ name: "artifacts" })}>
            Artifacts <span className="tally">{artifacts.length}</span>
          </button>
          <button className={view.name === "settings" ? "on" : ""} onClick={() => setView({ name: "settings" })}>
            Settings
          </button>
        </nav>
        <dl className="stats">
          <div>
            <dt>in review</dt>
            <dd>{counts.get("in_review") ?? 0}</dd>
          </div>
          <div>
            <dt>blocked</dt>
            <dd>{counts.get("blocked") ?? 0}</dd>
          </div>
          <div>
            <dt>done</dt>
            <dd>{counts.get("done") ?? 0}</dd>
          </div>
        </dl>
        <footer>
          <p>{project ? project.name : `${workspace.defaultPrefix} (no project)`}</p>
          <p className="dim">{project?.repos[0]?.vcs ?? "no repo"} · v{project?.repos[0]?.contractVersion ?? "?"}</p>
          <button
            className="link"
            onClick={() => {
              const next: Theme = theme === "system" ? "light" : theme === "light" ? "dark" : "system";
              setTheme(next);
              applyTheme(next);
            }}
          >
            theme: {theme}
          </button>
          <p className="dim">
            {connection?.label ?? "no server"}
            {savedRemote() ? (
              <>
                {" · "}
                <button
                  className="link"
                  onClick={() => {
                    useLocalServer();
                    setConnection(null);
                    setConnected(false);
                    setView({ name: "connect" });
                  }}
                >
                  local
                </button>
              </>
            ) : null}
          </p>
        </footer>
      </aside>

      <main className="stage">
        {notice && (
          <div className="notice" onClick={() => setNotice(null)} role="status">
            {notice}
          </div>
        )}
        {view.name === "skills" && workspace && <Skills workspaceId={workspace.id} />}
        {view.name === "workflow" && workspace && (
          <WorkflowScreen workspaceId={workspace.id} />
        )}
        {view.name === "board" && (
          <Board
            issues={shown}
            columns={workflow?.sections.map((section) => ({
              status: section.status,
              title: section.title,
              owner: section.owner,
            }))}
            onOpen={(id) => setView({ name: "issue", id })}
            onNew={() => setView({ name: "new" })}
            hasRepo={Boolean(project?.repos.length)}
          />
        )}
        {view.name === "issue" && openIssue && (
          <IssueDetail
            issue={openIssue}
            milestones={milestones}
            onBack={() => setView({ name: "board" })}
            onChanged={refresh}
          />
        )}
        {view.name === "issue" && !openIssue && <p className="hint">loading issue…</p>}
        {view.name === "new" && (
          <NewIssue
            workspaceId={workspace.id}
            projects={projects}
            onAddRepo={createRepo}
            onCreated={async (id) => {
              await refresh();
              setView({ name: "issue", id });
            }}
            onCancel={() => setView({ name: "board" })}
          />
        )}
        {view.name === "milestones" && (
          <Milestones
            workspaceId={workspace.id}
            projects={projects}
            milestones={milestones}
            issues={issues}
            artifacts={artifacts}
            onChanged={refresh}
          />
        )}
        {view.name === "artifacts" && (
          <Artifacts
            workspaceId={workspace.id}
            projects={projects}
            milestones={milestones}
            artifacts={artifacts}
            onChanged={refresh}
          />
        )}
        {view.name === "settings" && (
          <Settings
            workspace={workspace}
            workspaces={workspaces}
            onSwitchWorkspace={switchWorkspace}
            projects={projects}
            active={project}
            agents={agents}
            skills={skills}
            onChanged={async () => {
              await reload();
            }}
          />
        )}
      </main>
    </div>
  );
}
