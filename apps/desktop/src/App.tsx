import { useCallback, useEffect, useMemo, useState } from "react";
import { connect, gql, loadWorkspace, loadWorkspaceIssues, type Agent, type Connection, type Issue, type Project, type Workspace } from "./api";
import Board from "./screens/Board";
import IssueDetail from "./screens/IssueDetail";
import NewIssue from "./screens/NewIssue";
import Settings from "./screens/Settings";

type View = { name: "board" } | { name: "issue"; id: string } | { name: "new" } | { name: "settings" };

export default function App() {
  const [connection, setConnection] = useState<Connection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [project, setProject] = useState<Project | null>(null);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [issues, setIssues] = useState<Issue[]>([]);
  const [view, setView] = useState<View>({ name: "board" });
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!workspace) return;
    try {
      const next = await loadWorkspaceIssues(workspace.id);
      setIssues(next);
      setError(null);
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    }
  }, [workspace]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const info = await connect();
        if (cancelled) return;
        setConnection(info);
        const loaded = await loadWorkspace();
        if (cancelled) return;
        setWorkspace(loaded.workspace);
        setProject(loaded.project);
        setAgents(loaded.agents);
      } catch (cause) {
        setError(String(cause instanceof Error ? cause.message : cause));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 2500);
    return () => clearInterval(timer);
  }, [refresh]);

  const openIssue = issues.find((issue) => view.name === "issue" && issue.id === view.id) ?? null;
  const counts = useMemo(() => {
    const map = new Map<string, number>();
    for (const issue of issues) map.set(issue.status, (map.get(issue.status) ?? 0) + 1);
    return map;
  }, [issues]);

  async function createRepo(path: string) {
    if (!project) return;
    await gql(
      `mutation ($projectId: UUID!, $name: String!, $path: String!) { addRepo(projectId: $projectId, name: $name, path: $path) { id name } }`,
      { projectId: project.id, name: path.split("/").filter(Boolean).pop() ?? "repo", path },
    );
    const loaded = await loadWorkspace();
    setProject(loaded.project);
    setNotice("Repository added. Preflight the agents from Settings before starting work.");
  }

  if (error) {
    return (
      <main className="boot">
        <h1>hulahoop</h1>
        <p className="error">{error}</p>
        {connection && <p className="hint">server on port {connection.port}</p>}
      </main>
    );
  }

  if (!workspace) {
    return (
      <main className="boot">
        <h1>hulahoop</h1>
        <p className="hint">connecting to the local server…</p>
      </main>
    );
  }

  return (
    <div className="shell">
      <aside className="rail">
        <div className="brand">
          <span className="mark" aria-hidden="true" />
          hulahoop
        </div>
        <nav>
          <button className={view.name === "board" ? "active" : ""} onClick={() => setView({ name: "board" })}>
            Board <span className="tally">{issues.filter((i) => i.status !== "done" && i.status !== "cancelled").length}</span>
          </button>
          <button className={view.name === "new" ? "active" : ""} onClick={() => setView({ name: "new" })}>
            New issue
          </button>
          <button className={view.name === "settings" ? "active" : ""} onClick={() => setView({ name: "settings" })}>
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
          <p>{project ? project.name : "no project"}</p>
          <p className="dim">{project?.repos[0]?.vcs ?? "no repo"} · v{project?.repos[0]?.contractVersion ?? "?"}</p>
        </footer>
      </aside>

      <main className="stage">
        {notice && (
          <div className="notice" onClick={() => setNotice(null)} role="status">
            {notice}
          </div>
        )}
        {view.name === "board" && (
          <Board
            issues={issues}
            onOpen={(id) => setView({ name: "issue", id })}
            onNew={() => setView({ name: "new" })}
            hasRepo={Boolean(project?.repos.length)}
          />
        )}
        {view.name === "issue" && openIssue && (
          <IssueDetail issue={openIssue} onBack={() => setView({ name: "board" })} onChanged={refresh} />
        )}
        {view.name === "issue" && !openIssue && <p className="hint">loading issue…</p>}
        {view.name === "new" && project && (
          <NewIssue
            projectId={project.id}
            hasRepo={Boolean(project.repos.length)}
            onAddRepo={createRepo}
            onCreated={async (id) => {
              await refresh();
              setView({ name: "issue", id });
            }}
            onCancel={() => setView({ name: "board" })}
          />
        )}
        {view.name === "settings" && project && (
          <Settings project={project} agents={agents} onChanged={async () => {
            const loaded = await loadWorkspace();
            setProject(loaded.project);
            setAgents(loaded.agents);
          }} />
        )}
      </main>
    </div>
  );
}
