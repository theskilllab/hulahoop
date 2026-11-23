import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  connect,
  gql,
  loadArtifacts,
  loadMilestones,
  loadRecentRuns,
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
  type RecentRun,
  type Workspace,
  type Skill,
  type WorkflowDoc,
} from "./api";
import { invoke, WORKSPACE_KEY } from "./api";
import { applyTheme, resolvedTheme, savedPalette, themeOf, THEMES } from "./theme";
import Artifacts from "./screens/Artifacts";
import Agents from "./screens/Agents";
import Icon from "./Icon";
import Board from "./screens/Board";
import Connect from "./screens/Connect";
import Palette, { type PaletteTarget } from "./screens/Palette";
import IssueDetail from "./screens/IssueDetail";
import Milestones from "./screens/Milestones";
import Skills from "./screens/Skills";
import WorkflowScreen from "./screens/Workflow";
import Lumi from "./screens/Lumi";
import NewIssue from "./screens/NewIssue";
import Settings from "./screens/Settings";
import CreateWorkspace from "./screens/CreateWorkspace";
import { Connectors } from "./screens/Connectors";
import { Activity, Inbox, Projects, PullRequests } from "./screens/Misc";
import Cost from "./screens/Cost";

type View =
  | { name: "inbox" }
  | { name: "tasks" }
  | { name: "projects" }
  | { name: "milestones" }
  | { name: "artifacts" }
  | { name: "agents" }
  | { name: "skills" }
  | { name: "workflow" }
  | { name: "connectors" }
  | { name: "cost" }
  | { name: "prs" }
  | { name: "activity" }
  | { name: "issue"; id: string }
  | { name: "new" }
  | { name: "connect" };

const RAIL_KEY = "hulahoop.rail";

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
  const [runners, setRunners] = useState<RecentRun[]>([]);
  const [view, setView] = useState<View>({ name: "tasks" });
  const [connected, setConnected] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  /** `all` shows every project's tasks, including the ones filed under none. */
  const [projectFilter, setProjectFilter] = useState<string | "all">("all");
  const [collapsed, setCollapsed] = useState(localStorage.getItem(RAIL_KEY) === "collapsed");

  /// On macOS the traffic lights sit inside the folded rail's 48px, so they go
  /// with the labels. Everywhere else this is a no-op.
  useEffect(() => {
    invoke("set_traffic_lights", { visible: !collapsed }).catch(() => undefined);
  }, [collapsed]);
  const [wsMenu, setWsMenu] = useState(false);
  const [palette, setPalette] = useState(false);
  const [lumi, setLumi] = useState(false);
  const [settingsTab, setSettingsTab] = useState<"personal" | "workspace" | null>(null);
  const [creatingWorkspace, setCreatingWorkspace] = useState(false);
  const settingsReturnFocus = useRef<HTMLElement | null>(null);
  const openSettings = useCallback((tab: "personal" | "workspace" = "personal") => {
    settingsReturnFocus.current = document.activeElement as HTMLElement | null;
    setPalette(false);
    setSettingsTab(tab);
  }, []);
  const closeSettings = () => {
    setSettingsTab(null);
    requestAnimationFrame(() => {
      const target = settingsReturnFocus.current;
      if (target?.isConnected) target.focus();
      else document.querySelector<HTMLButtonElement>(".ws-btn")?.focus();
    });
  };

  // The page header's search field is the same door as ⌘K.
  useEffect(() => {
    const open = () => { if (!settingsTab) setPalette(true); };
    window.addEventListener("hulahoop:search", open);
    return () => window.removeEventListener("hulahoop:search", open);
  }, [settingsTab]);

  const refresh = useCallback(async () => {
    if (!workspace) return;
    try {
      const [nextIssues, nextMilestones, nextArtifacts, nextWorkflow, nextSkills, nextRuns] =
        await Promise.all([
          loadWorkspaceIssues(workspace.id),
          loadMilestones(workspace.id),
          loadArtifacts(workspace.id),
          loadWorkflow(workspace.id),
          loadSkills(workspace.id),
          loadRecentRuns(workspace.id, 150),
        ]);
      setIssues(nextIssues);
      setMilestones(nextMilestones);
      setArtifacts(nextArtifacts);
      setWorkflow(nextWorkflow);
      setSkills(nextSkills);
      setRunners(nextRuns);
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

  /** A different workspace means different projects, tasks and shelves. */
  const switchWorkspace = useCallback(
    async (id: string) => {
      localStorage.setItem(WORKSPACE_KEY, id);
      await load(id);
      setWsMenu(false);
      setView({ name: "tasks" });
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

  /** ⌘, opens settings, ⌥⌘S folds the rail — the two the prototype promises. */
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const meta = event.metaKey || event.ctrlKey;
      if (settingsTab) {
        if (meta && [",", "k", "n", "j", "s"].includes(event.key.toLowerCase())) event.preventDefault();
        return;
      }
      if (meta && event.key === ",") {
        event.preventDefault();
        openSettings();
      }
      if (meta && event.altKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        setCollapsed((was) => {
          localStorage.setItem(RAIL_KEY, was ? "expanded" : "collapsed");
          return !was;
        });
      }
      if (meta && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPalette((was) => !was);
      }
      if (meta && event.key.toLowerCase() === "n") {
        event.preventDefault();
        setView({ name: "new" });
      }
      if (meta && event.key.toLowerCase() === "j") {
        event.preventDefault();
        setLumi((was) => !was);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openSettings, settingsTab]);

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
    setNotice("Repository added. Probe the agents before starting work.");
  }

  if (view.name === "connect" || !connected) {
    return (
      <Connect
        failure={error}
        onCancel={connected ? () => setView({ name: "tasks" }) : undefined}
        onConnected={() => {
          setConnected(false);
          setView({ name: "tasks" });
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
        <h1>Hulahoop</h1>
        <p className="error">{error}</p>
        <button onClick={() => setView({ name: "connect" })}>connect to another server</button>
      </main>
    );
  }

  if (!workspace) {
    return (
      <main className="boot">
        <h1>Hulahoop</h1>
        <p className="hint">loading the workspace…</p>
      </main>
    );
  }

  const inboxCount = issues.filter((issue) => ["blocked", "in_review"].includes(issue.status)).length;
  const openTasks = issues.filter((issue) => !["done", "cancelled"].includes(issue.status)).length;
  const running = runners.filter((run) => run.status === "running").length;
  const openPrs = issues.filter((issue) => issue.prUrl && issue.status !== "done").length;

  const items: { name: View["name"]; icon: string; label: string; count?: number; hot?: boolean }[] = [
    { name: "inbox", icon: "inbox", label: "Inbox", count: inboxCount, hot: true },
    { name: "tasks", icon: "tasks", label: "Tasks", count: openTasks },
    { name: "projects", icon: "folder", label: "Projects" },
    { name: "milestones", icon: "flag", label: "Milestones", count: milestones.filter((m) => m.status === "open").length },
    { name: "artifacts", icon: "file", label: "Artifacts", count: artifacts.length },
    { name: "agents", icon: "bot", label: "Agents", count: running },
    { name: "skills", icon: "book", label: "Skills" },
    { name: "workflow", icon: "flow", label: "Workflow" },
    { name: "connectors", icon: "plug", label: "Connectors" },
    { name: "cost", icon: "coin", label: "Cost control" },
    { name: "prs", icon: "pr", label: "Pull requests", count: openPrs },
    { name: "activity", icon: "activity", label: "Activity" },
  ];

  const initial = workspaces.find((entry) => entry.id === workspace.id)?.name?.[0] ?? "H";

  /** What Lumi is told the person is looking at. */
  const lumiContext =
    view.name === "issue" && openIssue
      ? `Task ${openIssue.key} — ${openIssue.title}`
      : view.name === "tasks"
        ? "the board"
        : view.name;

  return (
    <div className={collapsed ? "shell collapsed" : "shell"}>
      <aside className={collapsed ? "rail collapsed" : "rail"}>
        <div className="brand" data-drag>
          {collapsed ? null : <Logo />}
          {collapsed ? null : <span className="name">Hulahoop</span>}
          <button
            className="ghost small rail-toggle"
            onClick={() => {
              setCollapsed((was) => {
                localStorage.setItem(RAIL_KEY, was ? "expanded" : "collapsed");
                return !was;
              });
            }}
            title={collapsed ? "Show the sidebar (⌥⌘S)" : "Hide the sidebar (⌥⌘S)"}
            aria-label={collapsed ? "Show the sidebar" : "Hide the sidebar"}
          >
            <Icon name="sidebar" size={15} />
          </button>
        </div>

        <nav>
          {items.map((item) => (
            <button
              key={item.name}
              className={view.name === item.name ? "on" : ""}
              onClick={() => setView({ name: item.name } as View)}
              title={collapsed ? item.label : undefined}
            >
              <Icon name={item.icon} />
              {!collapsed ? <span className="label">{item.label}</span> : null}
              {!collapsed && item.count ? (
                <span className={item.hot && item.count > 0 ? "tally hot" : "tally"}>{item.count}</span>
              ) : null}
            </button>
          ))}
        </nav>

        <footer className="ws-foot">
          <button
            className="ws-btn"
            onClick={() => setWsMenu(!wsMenu)}
            aria-haspopup="menu"
            aria-expanded={wsMenu}
            title={collapsed ? workspace.name : "Switch workspace"}
          >
            <span className="ws-av">{initial}</span>
            {!collapsed ? (
              <>
                <span className="ws-name">{workspace.name}</span>
                <Icon name="updown" size={13} />
              </>
            ) : null}
          </button>

          {wsMenu ? (
            <div className="ws-menu" role="menu">
              <div className="ws-head">Workspaces</div>
              {workspaces.map((entry) => (
                <button
                  key={entry.id}
                  className="ws-item"
                  role="menuitemradio"
                  aria-checked={entry.id === workspace.id}
                  onClick={() => void switchWorkspace(entry.id)}
                >
                  <span className="ws-av">{entry.name[0]}</span>
                  <span className="fill">{entry.name}</span>
                  {entry.id === workspace.id ? <Icon name="check" size={13} /> : null}
                </button>
              ))}
              <div className="ws-sep" />
              <button
                className="ws-item"
                role="menuitem"
                onClick={() => {
                  setWsMenu(false);
                  openSettings();
                }}
              >
                <Icon name="gear" size={14} />
                <span className="fill">Settings</span>
                <kbd>⌘,</kbd>
              </button>
              <button
                className="ws-item"
                role="menuitem"
                onClick={() => {
                  setWsMenu(false);
                  setPalette(false);
                  setCreatingWorkspace(true);
                }}
              >
                <Icon name="plus" size={14} />
                <span className="fill">Create workspace</span>
              </button>
              <div className="ws-sep" />
              <div className="ws-conn">
                {connection?.label ?? "this Mac"}
                <br />
                <span className="dim">{workspace.defaultPrefix} · issues without a project</span>
              </div>
              <button
                className="ws-item"
                role="menuitem"
                onClick={() => {
                  setWsMenu(false);
                  if (savedRemote()) {
                    useLocalServer();
                    setConnection(null);
                    setConnected(false);
                  }
                  setView({ name: "connect" });
                }}
              >
                <span className="fill">{savedRemote() ? "Use this Mac" : "Connect to a server…"}</span>
              </button>
              <div className="ws-sep" />
              <button
                className="ws-item"
                role="menuitem"
                onClick={() => {
                  setWsMenu(false);
                  // Hop to the other side of the current theme's family; the
                  // full choice of themes lives under Settings → Personal.
                  const current = themeOf(savedPalette());
                  const wantDark = resolvedTheme() === "light";
                  const next =
                    THEMES.find(
                      (theme) =>
                        theme.group === current.group &&
                        theme.kind === (wantDark ? "dark" : "light"),
                    ) ?? themeOf(wantDark ? "default-dark" : "default-light");
                  applyTheme(next.id);
                }}
              >
                <span className="fill">Switch appearance</span>
              </button>
            </div>
          ) : null}
        </footer>
      </aside>

      {palette ? (
        <Palette
          issues={issues}
          agents={agents}
          onClose={() => setPalette(false)}
          onPick={(target: PaletteTarget) => {
            setPalette(false);
            if (target.kind === "view") {
              if (target.id === "settings") openSettings();
              else setView({ name: target.id } as View);
            }
            if (target.kind === "issue") setView({ name: "issue", id: target.id });
            if (target.kind === "agent") setView({ name: "agents" });
          }}
        />
      ) : null}

      <main className="stage">
        {notice && (
          <div className="notice" onClick={() => setNotice(null)} role="status">
            {notice}
          </div>
        )}

        {view.name === "inbox" && (
          <Inbox issues={issues} onOpen={(id) => setView({ name: "issue", id })} />
        )}

        {view.name === "projects" && (
          <Projects
            projects={projects}
            issues={issues}
            workspace={workspace}
            onChanged={reload}
            onOpen={(id) => setView({ name: "issue", id })}
          />
        )}

        {view.name === "connectors" && <Connectors />}

        {view.name === "cost" && <Cost workspaceId={workspace.id} agents={agents} onOpen={(id) => setView({ name: "issue", id })} />}

        {view.name === "prs" && (
          <PullRequests issues={issues} onOpen={(id) => setView({ name: "issue", id })} />
        )}

        {view.name === "activity" && (
          <Activity
            runners={runners}
            issues={issues}
            agents={agents}
            onOpen={(id) => setView({ name: "issue", id })}
          />
        )}

        {view.name === "skills" && <Skills workspaceId={workspace.id} />}

        {view.name === "agents" && (
          <Agents
            workspaceId={workspace.id}
            agents={agents}
            skills={skills}
            issues={issues}
            repoId={projects.flatMap((entry) => entry.repos)[0]?.id ?? null}
            projectId={project?.id ?? null}
            onChanged={reload}
            onOpenSkills={() => setView({ name: "skills" })}
          />
        )}

        {view.name === "workflow" && <WorkflowScreen workspaceId={workspace.id} agents={agents} />}

        {view.name === "tasks" && (
          <>
            <div className="tasks-bar">
              <select
                value={projectFilter}
                onChange={(event) => setProjectFilter(event.target.value)}
                aria-label="Project"
              >
                <option value="all">all projects</option>
                {projects.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.prefix ? `${entry.prefix} · ${entry.name}` : entry.name}
                  </option>
                ))}
              </select>
              <span className="dim">{shown.filter((issue) => !["done", "cancelled"].includes(issue.status)).length} open</span>
              <span className="fill" />
              <button className="primary" onClick={() => setView({ name: "new" })}>
                <Icon name="plus" size={14} /> New task
              </button>
            </div>
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
          </>
        )}

        {view.name === "issue" && openIssue && (
          <IssueDetail
            issue={openIssue}
            milestones={milestones}
            onBack={() => setView({ name: "tasks" })}
            onChanged={refresh}
          />
        )}
        {view.name === "issue" && !openIssue && <p className="hint">loading task…</p>}

        {view.name === "new" && (
          <NewIssue
            workspaceId={workspace.id}
            projects={projects}
            onAddRepo={createRepo}
            onCreated={async (id) => {
              await refresh();
              setView({ name: "issue", id });
            }}
            onCancel={() => setView({ name: "tasks" })}
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

      </main>

      {settingsTab && (
        <Settings
          key={`${workspace.id}-${settingsTab}`}
          workspace={workspace}
          initialTab={settingsTab}
          onChanged={reload}
          onClose={closeSettings}
        />
      )}

      {creatingWorkspace && (
        <CreateWorkspace
          onCreated={switchWorkspace}
          onClose={() => setCreatingWorkspace(false)}
        />
      )}

      <Lumi
        workspace={workspace}
        context={lumiContext}
        open={lumi}
        onToggle={setLumi}
        onApplied={() => void refresh()}
        onOpenSettings={() => openSettings("workspace")}
      />
    </div>
  );
}
