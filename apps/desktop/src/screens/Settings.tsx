import { useEffect, useState } from "react";
import { gql, type Agent, type Project, type Repo, type Workspace } from "../api";

const ROLES = ["test_author", "implementer", "reviewer"] as const;
const ADAPTERS = ["pi", "claude_code", "fake"] as const;

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

export default function Settings({
  workspace,
  projects,
  active,
  agents,
  onChanged,
}: {
  workspace: Workspace;
  projects: Project[];
  active: Project | null;
  agents: Agent[];
  onChanged: () => Promise<void>;
}) {
  const [bindings, setBindings] = useState<Record<string, string>>({});
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [chosen, setChosen] = useState<string>(active?.id ?? "");
  const [prefix, setPrefix] = useState<string>(active?.prefix ?? "");
  const [newProject, setNewProject] = useState("");
  const [newPrefix, setNewPrefix] = useState("");
  const [repoPath, setRepoPath] = useState("");

  const project = projects.find((entry) => entry.id === chosen) ?? active ?? projects[0] ?? null;
  const repo: Repo | undefined = project?.repos[0];

  async function loadBindings() {
    if (!project) return;
    const data = await gql<{ projectAgents: Binding[] }>(
      `query ($id: UUID!) { projectAgents(projectId: $id) { role agentId } }`,
      { id: project.id },
    );
    const next: Record<string, string> = {};
    for (const binding of data.projectAgents) next[binding.role] = binding.agentId;
    setBindings(next);
  }

  async function loadPreflight() {
    if (!project) return;
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
  }, [project?.id]);

  /** A new harness or model invalidates its probe, so ask for a fresh one. */
  async function saveAgent(id: string, adapter: string, model: string) {
    await action("agent", async () => {
      await gql(`mutation ($id: UUID!, $adapter: String!, $model: String!) { updateAgent(id: $id, adapter: $adapter, model: $model) { id } }`, {
        id,
        adapter,
        model,
      });
      if (repo) {
        await gql(`mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }`, {
          agentId: id,
          repoId: repo.id,
        });
      }
    });
  }

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
                <strong>{entry.name}</strong> <span className="dim">{entry.path}</span>
                <div className="dim small">
                  {entry.vcs} · contract v{entry.contractVersion} · base {entry.baseBookmark} · fetched{" "}
                  {entry.fetchedAt ? new Date(entry.fetchedAt).toLocaleString() : "never"}
                </div>
              </li>
            ))}
            {project.repos.length === 0 && <li className="dim">No repository attached yet.</li>}
          </ul>
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

      <h2 className="section">GitHub</h2>
      <GithubConnector onChanged={onChanged} onError={setError} />

      <h2 className="section">Agent credentials</h2>
      <p className="dim small">
        A subscription is the credentials file the CLI wrote when you logged in on this machine; an API key is the
        alternative. Either way the run gets its own copy in a scratch home, so your login is never touched.
      </p>
      {AGENT_CONNECTORS.map((connector) => (
        <AgentConnector key={connector.provider} {...connector} onChanged={onChanged} onError={setError} />
      ))}

      <h2 className="section">Agents</h2>
      <ul className="agents">
        {agents.map((agent) => {
          const probe = preflight.find((entry) => entry.agentId === agent.id && entry.repoId === repo?.id);
          return (
            <li key={agent.id}>
              <div>
                <strong>{agent.name}</strong>
                <div className="agent-edit">
                  <select
                    value={agent.adapter}
                    disabled={busy !== null}
                    onChange={(e) => void saveAgent(agent.id, e.target.value, agent.model)}
                  >
                    {ADAPTERS.map((adapter) => (
                      <option key={adapter} value={adapter}>
                        {adapter}
                      </option>
                    ))}
                  </select>
                  <input
                    defaultValue={agent.model}
                    key={`${agent.id}-${agent.model}`}
                    placeholder="model"
                    onBlur={(e) => {
                      const model = e.target.value.trim();
                      if (model !== agent.model) void saveAgent(agent.id, agent.adapter, model);
                    }}
                  />
                </div>
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
      {!project && <p className="dim">Create a project before binding roles.</p>}
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
                      projectId: project!.id,
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

interface StoredConnection {
  provider: string;
  kind: string;
  connected: boolean;
  appId: string | null;
  slug: string | null;
  label: string | null;
  connectedAt: string | null;
}

/** The agent connectors, in the order the screen shows them. */
const AGENT_CONNECTORS: { provider: string; title: string; file: string; keyHint: string }[] = [
  {
    provider: "claude_code",
    title: "Claude Code subscription",
    file: ".claude/.credentials.json",
    keyHint: "sk-ant-…",
  },
  { provider: "codex", title: "Codex subscription", file: ".codex/auth.json", keyHint: "sk-…" },
  { provider: "pi", title: "Pi subscription", file: ".pi/agent/auth.json", keyHint: "provider key" },
];

/**
 * The GitHub App is what lets the loop commit as itself instead of as you. Two
 * states matter and they are different: nothing stored, and something stored that
 * cannot sign. The second one is why the panel reports a reason rather than a
 * tick.
 */
/**
 * One agent connector: a subscription file or an API key, stored sealed, copied
 * into each run's scratch home. Pasted values are never shown again, so the two
 * fields start empty and the stored state is what the row reports.
 */
function AgentConnector({
  provider,
  title,
  file,
  keyHint,
  onChanged,
  onError,
}: {
  provider: string;
  title: string;
  file: string;
  keyHint: string;
  onChanged: () => Promise<void>;
  onError: (message: string | null) => void;
}) {
  const [stored, setStored] = useState<StoredConnection | null>(null);
  const [contents, setContents] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  async function load() {
    const data = await gql<{ connections: StoredConnection[] }>(
      `query { connections { provider kind connected label connectedAt } }`,
    );
    setStored(data.connections.find((connection) => connection.provider === provider) ?? null);
  }

  useEffect(() => {
    void load();
  }, [provider]);

  async function run(label: string, work: () => Promise<unknown>) {
    setBusy(label);
    onError(null);
    try {
      await work();
      setContents("");
      setKey("");
      await load();
      await onChanged();
    } catch (failure) {
      onError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  const when = stored?.connectedAt ? `stored ${new Date(stored.connectedAt).toLocaleString()}` : "";

  return (
    <div className="settings-block">
      <div className="row spread">
        <strong>{title}</strong>
        {stored ? (
          <span className={stored.connected ? "ok small" : "bad small"}>
            {stored.connected ? `connected · ${stored.kind.replace("_", " ")}` : "stored, unusable"}
          </span>
        ) : (
          <span className="dim small">not stored</span>
        )}
      </div>
      <p className="dim small">
        {stored?.label ? `${stored.label} · ${when}` : `paste ${file} or an api key · ${when}`}
      </p>
      <label>
        Subscription file ({file})
        <textarea
          rows={3}
          value={contents}
          placeholder='{"claudeAiOauth":{"accessToken":"…"}}'
          onChange={(event) => setContents(event.target.value)}
        />
      </label>
      <label>
        API key
        <input value={key} placeholder={keyHint} onChange={(event) => setKey(event.target.value)} />
      </label>
      <div className="row">
        <button
          disabled={busy !== null || contents.trim().length < 2}
          onClick={() =>
            run("subscription", () =>
              gql(
                `mutation ($provider: String!, $contents: String!) { setAgentSubscription(provider: $provider, contents: $contents) { provider connected } }`,
                { provider, contents },
              ),
            )
          }
        >
          Save subscription
        </button>
        <button
          disabled={busy !== null || key.trim().length < 4}
          onClick={() =>
            run("key", () =>
              gql(
                `mutation ($provider: String!, $key: String!) { setAgentKey(provider: $provider, key: $key) { provider connected } }`,
                { provider, key },
              ),
            )
          }
        >
          Save key
        </button>
        {stored && (
          <button
            className="ghost"
            disabled={busy !== null}
            onClick={() =>
              run("disconnect", () =>
                gql(
                  `mutation ($provider: String!) { disconnectConnection(provider: $provider) { provider connected } }`,
                  { provider },
                ),
              )
            }
          >
            Disconnect
          </button>
        )}
      </div>
    </div>
  );
}

function GithubConnector({ onChanged, onError }: { onChanged: () => Promise<void>; onError: (message: string | null) => void }) {
  const [stored, setStored] = useState<StoredConnection | null>(null);
  const [appId, setAppId] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [busy, setBusy] = useState<string | null>(null);

  async function load() {
    const data = await gql<{ connections: StoredConnection[] }>(
      `query { connections { provider connected appId slug connectedAt } }`,
    );
    setStored(data.connections.find((connection) => connection.provider === "github") ?? null);
  }

  useEffect(() => {
    void load();
  }, []);

  async function run(label: string, work: () => Promise<unknown>) {
    setBusy(label);
    onError(null);
    try {
      await work();
      await load();
      await onChanged();
    } catch (failure) {
      onError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  const ready = appId.trim().length > 0 && privateKey.trim().length > 0;

  const appName = stored?.slug ? `${stored.slug} (${stored.appId ?? "unknown id"})` : stored?.appId;

  return (
    <div className="settings-block">
      {stored ? (
        <p>
          {stored.connected ? (
            <>
              <strong>connected</strong>{" "}
              {appName ? <strong>{appName}</strong> : null}{" "}
              <span className="dim small">
                the loop commits as its own app identity, and the key can sign
                {stored.connectedAt ? ` · stored ${new Date(stored.connectedAt).toLocaleString()}` : ""}
              </span>
            </>
          ) : (
            <>
              <strong className="error">not connected</strong>{" "}
              <span className="dim small">
                a key is stored and it cannot sign, so the loop would fall back to your own name. Paste the .pem again.
              </span>
            </>
          )}
        </p>
      ) : (
        <p className="dim">
          No GitHub app yet. Create one under GitHub → Settings → Developer settings → GitHub Apps, give it contents and pull
          request write access, then paste its id and its .pem here.
        </p>
      )}

      <label>
        App id
        <input value={appId} placeholder="123456" onChange={(event) => setAppId(event.target.value)} />
      </label>
      <label>
        Private key (.pem)
        <textarea
          rows={4}
          value={privateKey}
          placeholder="-----BEGIN RSA PRIVATE KEY-----"
          onChange={(event) => setPrivateKey(event.target.value)}
        />
      </label>
      <div className="row">
        <button
          disabled={busy !== null || !ready}
          onClick={() =>
            run("connect", () =>
              gql(
                `mutation ($appId: String!, $privateKey: String!) { createGithubApp(appId: $appId, privateKey: $privateKey) { provider connected } }`,
                { appId: appId.trim(), privateKey },
              ),
            )
          }
        >
          {stored ? "Replace the key" : "Connect"}
        </button>
        {stored && (
          <button
            disabled={busy !== null}
            onClick={() => {
              setPrivateKey("");
              void run("disconnect", () => gql(`mutation { disconnectConnection(provider: "github") { connected } }`));
            }}
          >
            Disconnect
          </button>
        )}
      </div>
    </div>
  );
}
