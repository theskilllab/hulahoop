import { useEffect, useState } from "react";
import { gql, type Agent, type Project, type Repo } from "../api";

const ROLES = ["test_author", "implementer", "reviewer"] as const;

interface Binding {
  role: string;
  agentId: string;
}

interface Preflight {
  agentId: string;
  repoId: string;
  ok: boolean;
  detail: string;
  checkedAt: string;
}

export default function Settings({ project, agents, onChanged }: { project: Project; agents: Agent[]; onChanged: () => Promise<void> }) {
  const [bindings, setBindings] = useState<Record<string, string>>({});
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const repo: Repo | undefined = project.repos[0];

  async function loadBindings() {
    const data = await gql<{ projectAgents: Binding[] }>(
      `query ($id: UUID!) { projectAgents(projectId: $id) { role agentId } }`,
      { id: project.id },
    );
    const next: Record<string, string> = {};
    for (const binding of data.projectAgents) next[binding.role] = binding.agentId;
    setBindings(next);
  }

  async function loadPreflight() {
    const data = await gql<{ preflight: Preflight[] }>(
      `query ($id: UUID!) { preflight(projectId: $id) { agentId repoId ok detail checkedAt } }`,
      { id: project.id },
    );
    setPreflight(data.preflight);
  }

  useEffect(() => {
    void loadBindings();
    void loadPreflight();
    const timer = setInterval(() => void loadPreflight(), 4000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.id]);

  async function action(label: string, work: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    try {
      await work();
      await onChanged();
      await loadPreflight();
      await loadBindings();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="detail">
      <h1>Settings</h1>
      {error && <p className="error">{error}</p>}

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

      <h2 className="section">Agents</h2>
      <ul className="agents">
        {agents.map((agent) => {
          const probe = preflight.find((entry) => entry.agentId === agent.id && entry.repoId === repo?.id);
          return (
            <li key={agent.id}>
              <div>
                <strong>{agent.name}</strong>
                <p className="dim small">
                  {agent.adapter} · {agent.model}
                </p>
              </div>
              <div className="probe">
                {probe ? (
                  <span className={probe.ok ? "ok" : "bad"} title={probe.detail}>
                    {probe.ok ? "preflight green" : "preflight failed"}
                  </span>
                ) : (
                  <span className="dim">no probe yet</span>
                )}
                <button
                  disabled={busy !== null || !repo}
                  onClick={() =>
                    repo &&
                    action("probe", () =>
                      gql(`mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }`, {
                        agentId: agent.id,
                        repoId: repo.id,
                      }),
                    )
                  }
                >
                  Run probe
                </button>
              </div>
            </li>
          );
        })}
      </ul>

      <h2 className="section">Roles</h2>
      <p className="dim small">
        A Test Author and an Implementer on the same model family are weaker than they look: the tests and the code share the same
        blind spots.
      </p>
      <div className="roles">
        {ROLES.map((role) => (
          <label key={role}>
            {role.replaceAll("_", " ")}
            <select
              value={bindings[role] ?? ""}
              onChange={(e) => {
                const next = { ...bindings, [role]: e.target.value };
                setBindings(next);
                void action("bind", () =>
                  gql(
                    `mutation ($projectId: UUID!, $bindings: [RoleBinding!]!) { setProjectAgents(projectId: $projectId, bindings: $bindings) { role } }`,
                    {
                      projectId: project.id,
                      bindings: ROLES.filter((r) => next[r]).map((r) => ({ role: r, agentId: next[r] })),
                    },
                  ),
                );
              }}
            >
              <option value="">unassigned</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name}
                </option>
              ))}
            </select>
          </label>
        ))}
      </div>
    </section>
  );
}
