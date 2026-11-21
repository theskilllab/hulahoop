import { useEffect, useMemo, useState } from "react";

import {
  gql,
  loadModels,
  setAgentInstructions,
  setAgentSkills,
  type Agent,
  type Issue,
  type ModelOption,
  type Skill,
} from "../api";
import Icon from "../Icon";

/** Harnesses we can run. `harness` is the provider key a connection is stored under. */
const HARNESSES: { adapter: string; harness: string; name: string }[] = [
  { adapter: "pi", harness: "pi", name: "Pi" },
  { adapter: "claude_code", harness: "claude_code", name: "Claude Code" },
  { adapter: "fake", harness: "", name: "Fake (development)" },
];

interface Connection {
  provider: string;
  kind: string;
  connected: boolean;
  label: string | null;
  connectedAt: string | null;
}

interface Preflight {
  agentId: string;
  repoId: string;
  ok: boolean;
  detail: string;
  checkedAt: string;
}

const harnessOf = (adapter: string) => HARNESSES.find((entry) => entry.adapter === adapter);
const statusOf = (running: boolean): { label: string; tone: string } =>
  running ? { label: "Running", tone: "ok" } : { label: "Idle", tone: "" };

/**
 * Agents are the workers: a name, a harness, a model, the instructions it works by and
 * the skills it carries. The workflow document picks among them by name, so this screen
 * is where a team is assembled — a roster on the left, one agent on the right.
 */
export default function Agents({
  workspaceId,
  agents,
  skills,
  issues,
  repoId,
  projectId,
  onChanged,
}: {
  workspaceId: string;
  agents: Agent[];
  skills: Skill[];
  issues: Issue[];
  repoId: string | null;
  projectId: string | null;
  onChanged: () => Promise<void>;
}) {
  const [selected, setSelected] = useState<string | null>(agents[0]?.id ?? null);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [editing, setEditing] = useState<null | "new" | string>(null);
  const [tab, setTab] = useState<"diff" | "tx" | "log">("diff");
  const [error, setError] = useState<string | null>(null);

  async function loadConnections() {
    const data = await gql<{ connections: Connection[] }>(
      `query { connections { provider kind connected label connectedAt } }`,
    );
    setConnections(data.connections);
  }

  async function loadPreflight() {
    if (!projectId) return;
    try {
      const data = await gql<{ preflight: Preflight[] }>(
        `query ($p: UUID!) { preflight(projectId: $p) { agentId repoId ok detail checkedAt } }`,
        { p: projectId },
      );
      setPreflight(data.preflight);
    } catch {
      /* a workspace without a repository has nothing to probe yet */
    }
  }

  useEffect(() => {
    void loadConnections().catch(() => setConnections([]));
  }, [workspaceId]);

  useEffect(() => {
    void loadPreflight();
  }, [projectId, agents.length]);

  useEffect(() => {
    if (!selected && agents.length) setSelected(agents[0].id);
    if (selected && !agents.some((agent) => agent.id === selected)) setSelected(agents[0]?.id ?? null);
  }, [agents, selected]);

  const agent = agents.find((entry) => entry.id === selected) ?? null;
  const connected = (harness: string) =>
    harness === "" || connections.some((entry) => entry.provider === harness && entry.connected);

  /** The newest run this agent started. Every run records whose time it was. */
  const current = useMemo(() => {
    if (!agent) return null;
    const flat = issues.flatMap((issue) => issue.runs.map((run) => ({ issue, run })));
    const mine = flat
      .filter((entry) => entry.run.agentId === agent.id)
      .sort((a, b) => (a.run.startedAt ?? "").localeCompare(b.run.startedAt ?? ""));
    return mine.at(-1) ?? null;
  }, [agent, issues]);

  const running = current?.run.status === "running";

  /** Which harnesses have a live run right now — the roster's status chip reads it. */
  const runningAgents = new Set(
    issues.flatMap((issue) =>
      issue.runs.filter((run) => run.status === "running").map((run) => run.agentId),
    ),
  );

  return (
    <section className="screen agents-screen">
      <header className="bar">
        <h1>Agents</h1>
        <div className="row">
          <span className="dim small">
            {agents.filter((entry) => runningAgents.has(entry.id)).length} running · local runner
          </span>
          <button className="primary" onClick={() => setEditing("new")}>
            <Icon name="plus" size={14} /> New agent
          </button>
        </div>
      </header>

      {error ? <p className="error">{error}</p> : null}

      <div className="split">
        <div className="roster">
          {agents.map((entry) => {
            const isRunning = runningAgents.has(entry.id);
            const link = harnessOf(entry.adapter);
            const lost = !connected(link?.harness ?? "");
            return (
              <button
                key={entry.id}
                className="acard"
                aria-selected={entry.id === selected}
                onClick={() => setSelected(entry.id)}
              >
                <span className="row spread">
                  <span className="avatar">
                    <Icon name="bot" size={14} />
                  </span>
                  <strong className="fill truncate">{entry.name}</strong>
                  <span className={statusOf(isRunning).tone === "ok" ? "chip ok" : "chip"}>
                    {isRunning ? <span className="pulse" /> : null}
                    {statusOf(isRunning).label}
                  </span>
                </span>
                <span className="truncate small dim">
                  {link?.name ?? entry.adapter} · {entry.model || "default model"}
                  {entry.skills.length ? ` · ${entry.skills.length} skill${entry.skills.length === 1 ? "" : "s"}` : ""}
                </span>
                <span className={lost ? "truncate tiny warn" : "truncate tiny dim"}>
                  {lost ? `${link?.name} is not connected` : (current?.issue.key ?? "No task assigned")}
                </span>
              </button>
            );
          })}
          {agents.length === 0 ? (
            <p className="dim small">No agents yet. Create one, then give it a harness that has a connection.</p>
          ) : null}
        </div>

        {agent ? (
          <div className="agent-pane">
            <div className="ahead">
              <div className="row spread">
                <span className="avatar">
                  <Icon name="bot" size={16} />
                </span>
                <h2 className="fill truncate">{agent.name}</h2>
                <span className={running ? "chip ok" : "chip"}>
                  {running ? <span className="pulse" /> : null}
                  {running ? "Running" : "Idle"}
                </span>
              </div>

              <div className="pillrow">
                <span className="chip">{harnessOf(agent.adapter)?.name ?? agent.adapter}</span>
                <span className="chip">{agent.model || "default model"}</span>
                {connected(harnessOf(agent.adapter)?.harness ?? "") ? null : (
                  <span className="chip warn">Not connected</span>
                )}
                <button className="linkbtn" onClick={() => setEditing(agent.id)}>
                  AGENTS.md
                </button>
                {agent.skills.slice(0, 4).map((skill) => (
                  <span className="chip" key={skill.id} title="Skill">
                    {skill.name}
                  </span>
                ))}
                {agent.skills.length > 4 ? <span className="chip">+{agent.skills.length - 4}</span> : null}
              </div>

              <div className="pillrow">
                {current ? (
                  <>
                    <span className="chip info mono">{current.issue.key}</span>
                    <span className="truncate">{current.issue.title}</span>
                    {current.run.headBookmark ? (
                      <span className="chip mono">
                        <Icon name="branch" size={12} /> {current.run.headBookmark}
                      </span>
                    ) : null}
                  </>
                ) : (
                  <span className="dim small">No task assigned. Planning a task in To do hands it to an agent.</span>
                )}
              </div>

              <div className="row">
                <button onClick={() => setEditing(agent.id)}>
                  <Icon name="gear" size={14} /> Edit agent
                </button>
                {repoId && projectId ? (
                  <button
                    onClick={async () => {
                      await gql(
                        `mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }`,
                        { agentId: agent.id, repoId },
                      );
                      setTimeout(() => void loadPreflight(), 6000);
                    }}
                  >
                    <Icon name="refresh" size={14} /> Probe
                  </button>
                ) : null}
                {preflight.find((entry) => entry.agentId === agent.id) ? (
                  <span
                    className={
                      preflight.find((entry) => entry.agentId === agent.id)!.ok ? "ok small" : "bad small"
                    }
                  >
                    {preflight.find((entry) => entry.agentId === agent.id)!.ok
                      ? `harness answers (${preflight.find((entry) => entry.agentId === agent.id)!.detail})`
                      : `probe failed: ${preflight.find((entry) => entry.agentId === agent.id)!.detail}`}
                  </span>
                ) : null}
              </div>
            </div>

            <div className="tabs" role="tablist">
              <button role="tab" aria-selected={tab === "diff"} onClick={() => setTab("diff")}>
                Live diff
              </button>
              <button role="tab" aria-selected={tab === "tx"} onClick={() => setTab("tx")}>
                Transcript
              </button>
              <button role="tab" aria-selected={tab === "log"} onClick={() => setTab("log")}>
                Activity log
              </button>
            </div>

            <div className="tab-body">
              {tab === "diff" ? (
                <div className="empty">
                  <strong>No changes to show</strong>
                  <span className="dim small">
                    A run's diff streams here once the run starts. Task runs are shown on the task, not
                    here.
                  </span>
                </div>
              ) : null}
              {tab === "tx" ? (
                <div className="empty">
                  <strong>Transcript</strong>
                  <span className="dim small">
                    The agent's own words for its current run. Open the task to read the full transcript
                    of a finished one.
                  </span>
                </div>
              ) : null}
              {tab === "log" ? (
                <div className="empty">
                  <strong>Activity log</strong>
                  <span className="dim small">
                    Every run this agent started, with its outcome, will be listed here.
                  </span>
                </div>
              ) : null}
            </div>
          </div>
        ) : (
          <div className="empty">
            <strong>No agent selected</strong>
            <span className="dim small">Pick one on the left, or create one.</span>
          </div>
        )}
      </div>

      {editing ? (
        <AgentDialog
          workspaceId={workspaceId}
          agent={editing === "new" ? null : (agents.find((entry) => entry.id === editing) ?? null)}
          skills={skills}
          connected={connected}
          onClose={() => setEditing(null)}
          onError={setError}
          onSaved={async (id) => {
            await onChanged();
            if (id) setSelected(id);
            setEditing(null);
          }}
        />
      ) : null}
    </section>
  );
}

/** The edit form: name, harness (only what is connected), model, AGENTS.md, skills. */
function AgentDialog({
  workspaceId,
  agent,
  skills,
  connected,
  onClose,
  onSaved,
  onError,
}: {
  workspaceId: string;
  agent: Agent | null;
  skills: Skill[];
  connected: (harness: string) => boolean;
  onClose: () => void;
  onSaved: (id: string | null) => Promise<void>;
  onError: (message: string | null) => void;
}) {
  const [name, setName] = useState(agent?.name ?? "");
  const [adapter, setAdapter] = useState(agent?.adapter ?? HARNESSES[0].adapter);
  const [model, setModel] = useState(agent?.model ?? "");
  const [md, setMd] = useState(agent?.agentsMd ?? "");
  const [picked, setPicked] = useState<string[]>(agent?.skills.map((skill) => skill.id) ?? []);
  const [busy, setBusy] = useState<string | null>(null);

  const harness = harnessOf(adapter) ?? HARNESSES[0];
  const [models, setModels] = useState<ModelOption[]>([]);

  // The list comes from the harness, not from us: pi reports the providers it is
  // signed in to, Claude Code reports its aliases.
  useEffect(() => {
    let live = true;
    loadModels(adapter)
      .then((next) => {
        if (!live) return;
        setModels(next);
        if (next.length && !next.some((entry) => entry.id === model)) setModel(next[0].id);
      })
      .catch(() => live && setModels([]));
    return () => {
      live = false;
    };
  }, [adapter]);

  const providers = useMemo(() => {
    const groups = new Map<string, ModelOption[]>();
    for (const entry of models) {
      groups.set(entry.provider, [...(groups.get(entry.provider) ?? []), entry]);
    }
    return [...groups.entries()];
  }, [models]);

  async function save() {
    setBusy("save");
    onError(null);
    try {
      let id = agent?.id ?? null;
      if (agent) {
        await gql(
          `mutation ($id: UUID!, $name: String!, $adapter: String!, $model: String) {
             updateAgent(id: $id, name: $name, adapter: $adapter, model: $model) { id }
           }`,
          { id: agent.id, name, adapter, model },
        );
      } else {
        const data = await gql<{ createAgent: { id: string } }>(
          `mutation ($w: UUID!, $n: String!, $a: String!, $m: String) {
             createAgent(workspaceId: $w, name: $n, adapter: $a, model: $m) { id }
           }`,
          { w: workspaceId, n: name, a: adapter, m: model },
        );
        id = data.createAgent.id;
      }
      if (id) {
        await setAgentInstructions(id, md);
        await setAgentSkills(id, picked);
      }
      await onSaved(id);
    } catch (failure) {
      onError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="dialog" role="dialog" aria-modal="true" onClick={onClose}>
      <div
        className="dialog-card"
        onClick={(event) => {
          event.stopPropagation();
        }}
      >
        <header className="row spread">
          <h2>{agent ? `Edit ${agent.name}` : "New agent"}</h2>
          <button className="ghost" onClick={onClose} aria-label="Close">
            <Icon name="close" size={14} />
          </button>
        </header>

        <label>
          Name
          <input value={name} onChange={(event) => setName(event.target.value)} autoFocus />
        </label>

        <label>
          Harness
          <select
            value={adapter}
            onChange={(event) => {
              setAdapter(event.target.value);
              setModel("");
            }}
          >
            {HARNESSES.map((entry) => {
              const usable = connected(entry.harness);
              return (
                <option key={entry.adapter} value={entry.adapter} disabled={!usable}>
                  {entry.name}
                  {usable ? "" : " — not connected"}
                </option>
              );
            })}
          </select>
        </label>
        <p className="dim small">
          {connected(harness.harness)
            ? `${harness.name} runs on your ${harness.name} connection.`
            : `${harness.name} has no connection yet. Connect it on the Connectors screen, then come back.`}
        </p>

        <label>
          Model
          {models.length > 0 ? (
            <select
              value={model}
              onChange={(event) => setModel(event.target.value)}
              disabled={!connected(harness.harness)}
            >
              {providers.map(([provider, entries]) => (
                <optgroup key={provider} label={provider}>
                  {entries.map((entry) => (
                    <option key={entry.id} value={entry.id}>
                      {entry.label}
                      {entry.costOut !== null ? ` — $${entry.costIn ?? 0}/$${entry.costOut} per Mtok` : ""}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          ) : (
            <input
              value={model}
              placeholder="provider/model"
              onChange={(event) => setModel(event.target.value)}
            />
          )}
        </label>
        <p className="dim small">
          {models.length > 0
            ? `${models.length} models from ${harness.name}'s own catalogue.`
            : `${harness.name} did not report a catalogue, so type the model it takes.`}
        </p>

        <label>
          AGENTS.md
          <textarea rows={8} value={md} onChange={(event) => setMd(event.target.value)} />
        </label>
        <p className="dim small">Read on every run, before the skills. Keep it short: how this agent works.</p>

        <div className="skills-pick">
          {skills.length === 0 ? (
            <p className="dim small">No skills in this workspace yet. Create one on the Skills screen.</p>
          ) : (
            skills.map((skill) => {
              const on = picked.includes(skill.id);
              return (
                <button
                  key={skill.id}
                  className={on ? "sk on" : "sk"}
                  onClick={() =>
                    setPicked(on ? picked.filter((id) => id !== skill.id) : [...picked, skill.id])
                  }
                  aria-pressed={on}
                >
                  <Icon name="book" size={12} />
                  {skill.name}
                </button>
              );
            })
          )}
        </div>

        <footer className="row spread">
          <div className="row">
            {agent ? (
              <button
                className="danger"
                disabled={busy !== null}
                onClick={async () => {
                  const yes = confirm(`Delete ${agent.name}? Runs it started stay in the history.`);
                  if (!yes) return;
                  await gql(`mutation ($id: UUID!) { deleteAgent(id: $id) }`, { id: agent.id });
                  await onSaved(null);
                }}
              >
                Delete agent
              </button>
            ) : null}
          </div>
          <div className="row">
            <button onClick={onClose}>Cancel</button>
            <button
              className="primary"
              onClick={() => void save()}
              disabled={busy !== null || name.trim().length < 2 || !connected(harness.harness)}
            >
              {agent ? "Save agent" : "Create agent"}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
