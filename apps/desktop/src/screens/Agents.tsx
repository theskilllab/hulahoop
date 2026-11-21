import { useEffect, useState } from "react";

import {
  gql,
  setAgentInstructions,
  setAgentSkills,
  type Agent,
  type Skill,
} from "../api";
import Icon from "../Icon";

const ADAPTERS = ["pi", "claude_code", "fake"] as const;

interface Preflight {
  agentId: string;
  repoId: string;
  ok: boolean;
  detail: string;
  checkedAt: string;
}

/// Agents are the workers: a name, a harness, a model, the instructions it works
/// by and the skills it carries. The workflow document picks among these by name.
export default function Agents({
  workspaceId,
  agents,
  skills,
  repoId,
  projectId,
  onChanged,
}: {
  workspaceId: string;
  agents: Agent[];
  skills: Skill[];
  repoId: string | null;
  projectId: string | null;
  onChanged: () => Promise<void>;
}) {
  const [preflight, setPreflight] = useState<Preflight[]>([]);
  const [openId, setOpenId] = useState<string | null>(null);
  const [draft, setDraft] = useState({ name: "", adapter: "pi", model: "" });
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

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
    void loadPreflight();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId, agents.length]);

  async function run(key: string, work: () => Promise<unknown>) {
    setBusy(key);
    try {
      await work();
      await onChanged();
      await loadPreflight();
      setError(null);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="screen agents">
      <header>
        <div>
          <h1>Agents</h1>
          <p className="dim">
            The workers. Each one is a harness plus a model, its own instructions, and the skills it
            carries. The workflow names one of these per status.
          </p>
        </div>
        <button
          className="primary"
          onClick={() => setCreating(!creating)}
          aria-expanded={creating}
        >
          <Icon name={creating ? "close" : "plus"} /> {creating ? "Cancel" : "New agent"}
        </button>
      </header>

      {error ? <p className="error">{error}</p> : null}

      {creating ? (
        <form
          className="panel new-agent"
          onSubmit={(event) => {
            event.preventDefault();
            void run("create", async () => {
              await gql(
                `mutation ($w: UUID!, $n: String!, $a: String!, $m: String) {
                  createAgent(workspaceId: $w, name: $n, adapter: $a, model: $m) { id }
                }`,
                { w: workspaceId, n: draft.name.trim(), a: draft.adapter, m: draft.model.trim() || null },
              );
              setDraft({ name: "", adapter: "pi", model: "" });
              setCreating(false);
            });
          }}
        >
          <div className="row">
            <label className="stack">
              <span className="label">name</span>
              <input
                autoFocus
                placeholder="ada"
                value={draft.name}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            <label className="stack">
              <span className="label">harness</span>
              <select
                value={draft.adapter}
                onChange={(event) => setDraft({ ...draft, adapter: event.target.value })}
              >
                {ADAPTERS.map((adapter) => (
                  <option key={adapter} value={adapter}>
                    {adapter}
                  </option>
                ))}
              </select>
            </label>
            <label className="stack">
              <span className="label">model</span>
              <input
                placeholder="empty means the harness default"
                value={draft.model}
                onChange={(event) => setDraft({ ...draft, model: event.target.value })}
              />
            </label>
          </div>
          <button className="primary" disabled={busy !== null || !draft.name.trim()}>
            Create agent
          </button>
        </form>
      ) : null}

      <div className="agent-list">
        {agents.length === 0 ? (
          <p className="dim">No agents yet. Create one, and the workflow can send work to it.</p>
        ) : null}
        {agents.map((agent) => {
          const probe = preflight.find((entry) => entry.agentId === agent.id && entry.repoId === repoId);
          const isOpen = openId === agent.id;
          return (
            <article key={agent.id} className="agent-card">
              <header>
                <div className="agent-name">
                  <strong>{agent.name}</strong>
                  <span className="chip">{agent.adapter}</span>
                  {agent.model ? <span className="mono dim">{agent.model}</span> : null}
                </div>
                <div className="probe">
                  {probe ? (
                    <span className={probe.ok ? "ok" : "bad"} title={probe.detail}>
                      {probe.ok ? "ready" : "needs attention"}
                    </span>
                  ) : (
                    <span className="dim">not probed</span>
                  )}
                  <button
                    className="ghost small"
                    disabled={busy !== null || !repoId}
                    onClick={() =>
                      repoId &&
                      run(`probe-${agent.id}`, () =>
                        gql(
                          `mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }`,
                          { agentId: agent.id, repoId },
                        ),
                      )
                    }
                  >
                    Probe
                  </button>
                  <button className="link" onClick={() => setOpenId(isOpen ? null : agent.id)}>
                    {isOpen ? "close" : "edit"}
                  </button>
                </div>
              </header>

              {isOpen ? (
                <div className="stack">
                  <div className="row">
                    <label className="stack">
                      <span className="label">harness</span>
                      <select
                        value={agent.adapter}
                        onChange={(event) =>
                          void run(`adapter-${agent.id}`, () =>
                            gql(
                              `mutation ($id: UUID!, $adapter: String!, $model: String) {
                                updateAgent(id: $id, adapter: $adapter, model: $model) { id }
                              }`,
                              { id: agent.id, adapter: event.target.value, model: agent.model || null },
                            ),
                          )
                        }
                      >
                        {ADAPTERS.map((adapter) => (
                          <option key={adapter} value={adapter}>
                            {adapter}
                          </option>
                        ))}
                      </select>
                    </label>
                    <label className="stack">
                      <span className="label">model</span>
                      <input
                        defaultValue={agent.model}
                        key={`${agent.id}-${agent.model}`}
                        onBlur={(event) => {
                          const model = event.target.value.trim();
                          if (model !== agent.model) {
                            void run(`model-${agent.id}`, () =>
                              gql(
                                `mutation ($id: UUID!, $adapter: String!, $model: String) {
                                  updateAgent(id: $id, adapter: $adapter, model: $model) { id }
                                }`,
                                { id: agent.id, adapter: agent.adapter, model: model || null },
                              ),
                            );
                          }
                        }}
                      />
                    </label>
                  </div>

                  <label className="stack">
                    <span className="label">AGENTS.md — how this one works</span>
                    <textarea
                      rows={7}
                      defaultValue={agent.agentsMd}
                      placeholder="Read the crate you touch before you change it. Prefer small diffs. Ask before touching CI."
                      onBlur={(event) => {
                        const next = event.target.value;
                        if (next !== agent.agentsMd) {
                          void run(`md-${agent.id}`, () => setAgentInstructions(agent.id, next));
                        }
                      }}
                    />
                  </label>

                  <div className="stack">
                    <span className="label">skills it carries</span>
                    {skills.length === 0 ? (
                      <span className="dim">no skills in this workspace yet</span>
                    ) : (
                      <div className="skill-picks">
                        {skills.map((skill) => (
                          <label key={skill.id} className="check">
                            <input
                              type="checkbox"
                              checked={(agent.skills ?? []).some((carried) => carried.id === skill.id)}
                              onChange={(event) => {
                                const carried = (agent.skills ?? []).map((entry) => entry.id);
                                const next = event.target.checked
                                  ? [...carried, skill.id]
                                  : carried.filter((id) => id !== skill.id);
                                void run(`skills-${agent.id}`, () => setAgentSkills(agent.id, next));
                              }}
                            />
                            <span>{skill.name}</span>
                            <span className="dim">{skill.description}</span>
                          </label>
                        ))}
                      </div>
                    )}
                  </div>

                  <details className="danger-zone">
                    <summary>Delete this agent</summary>
                    <p className="dim">
                      Its runs stay in the history. A workflow that names it will refuse to dispatch
                      until you pick another.
                    </p>
                    <button
                      className="danger"
                      disabled={busy !== null}
                      onClick={() =>
                        void run(`delete-${agent.id}`, () =>
                          gql(`mutation ($id: UUID!) { deleteAgent(id: $id) }`, { id: agent.id }),
                        )
                      }
                    >
                      Delete {agent.name}
                    </button>
                  </details>
                </div>
              ) : (
                <p className="dim agent-line">
                  {agent.agentsMd ? "instructions written" : "no instructions yet"}
                  {" · "}
                  {(agent.skills ?? []).length} skills
                </p>
              )}
            </article>
          );
        })}
      </div>
    </section>
  );
}
