import { useEffect, useId, useMemo, useRef, useState, type FormEvent } from "react";

import {
  gql,
  loadConnections,
  loadModels,
  setAgentInstructions,
  setAgentSkills,
  type Agent,
  type ConnectorRow,
  type ModelOption,
  type Skill,
} from "../api";
import Select, { type SelectOption } from "../components/Select";
import { servesPiModel } from "../piProviders";
import { HARNESSES, harnessOf } from "./agentHarness";

const costOf = (entry: ModelOption) => (entry.costOut !== null ? `$${entry.costIn ?? 0} / $${entry.costOut}` : undefined);

/**
 * The one New agent / Edit agent modal: name, harness, model, the connector it signs
 * in with, AGENTS.md and skills. A native `<dialog>`, so Esc and the backdrop close it.
 */
export default function AgentDialog({
  workspaceId,
  agent,
  takenNames,
  skills,
  connected,
  onOpenSkills,
  onOpenConnectors,
  onClose,
  onSaved,
}: {
  workspaceId: string;
  agent: Agent | null;
  /** Names of the other agents; a name is unique ignoring case. */
  takenNames: string[];
  skills: Skill[];
  connected: (harness: string) => boolean;
  onOpenSkills: () => void;
  onOpenConnectors: () => void;
  onClose: () => void;
  /** `id` is null after a delete. */
  onSaved: (id: string | null, toast: string) => Promise<void>;
}) {
  const ids = { name: useId(), md: useId() };
  const dialog = useRef<HTMLDialogElement>(null);
  const nameBox = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(agent?.name ?? "");
  const [nameError, setNameError] = useState<string | null>(null);
  const [adapter, setAdapter] = useState(agent?.adapter ?? HARNESSES[0].adapter);
  const [model, setModel] = useState(agent?.model ?? "");
  // A harness with two ways to pay needs to know which one this agent uses.
  const [connector, setConnector] = useState(agent?.connector ?? "");
  // `null` until the list arrives, so a stored choice is not flagged before it can be checked.
  const [connectors, setConnectors] = useState<ConnectorRow[] | null>(null);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [md, setMd] = useState(agent?.agentsMd ?? "");
  const [picked, setPicked] = useState<string[]>(agent?.skills.map((skill) => skill.id) ?? []);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const harness = harnessOf(adapter) ?? HARNESSES[0];
  const usable = connected(harness.harness);

  useEffect(() => {
    dialog.current?.showModal();
    nameBox.current?.focus();
    nameBox.current?.select();
    return () => dialog.current?.close();
  }, []);

  // A Pi model from another provider makes a per-provider choice moot: the server
  // would ignore it, so the form lets it go instead of saving it.
  const chooseModel = (next: string) => {
    setModel(next);
    if (adapter === "pi") setConnector((current) => (current && !servesPiModel(current, next) ? "" : current));
  };

  // The list comes from the harness, not from us: pi reports the providers it is
  // signed in to, Claude Code reports its aliases.
  useEffect(() => {
    let live = true;
    loadModels(adapter)
      .then((next) => {
        if (!live) return;
        setModels(next);
        if (next.length && !next.some((entry) => entry.id === model)) chooseModel(next[0].id);
      })
      .catch(() => live && setModels([]));
    return () => {
      live = false;
    };
  }, [adapter]);

  useEffect(() => {
    void loadConnections()
      .then(setConnectors)
      .catch(() => setConnectors([]));
  }, []);

  // The connected entries the server would use for this harness and, on Pi, for
  // this model's provider (the Pi login serves any model).
  const offered = (connectors ?? []).filter(
    (row) =>
      row.connected && row.harnesses.includes(adapter) && (adapter !== "pi" || servesPiModel(row.id, model)),
  );
  // A stored choice that is not on offer stays visible, disabled, instead of reading as Automatic.
  const stored = connectors?.find((row) => row.id === connector);
  const stale = connectors !== null && connector !== "" && !offered.some((row) => row.id === connector);
  const staleReason =
    stored?.connected && stored.harnesses.includes(adapter) ? "does not serve this model" : "not connected";

  const harnessOptions: SelectOption[] = HARNESSES.filter(
    (entry) => entry.adapter !== "fake" || import.meta.env.DEV || entry.adapter === adapter,
  ).map((entry) => {
    const ok = connected(entry.harness);
    return { value: entry.adapter, label: ok ? entry.name : `${entry.name} (not connected)`, disabled: !ok };
  });

  const modelOptions = useMemo<SelectOption[]>(
    () => models.map((entry) => ({ value: entry.id, label: entry.label, group: entry.provider, detail: costOf(entry) })),
    [models],
  );

  const connectorOptions: SelectOption[] = [
    { value: "", label: "Automatic" },
    ...offered.map((row) => ({ value: row.id, label: row.name })),
    ...(stale ? [{ value: connector, label: `${stored?.name ?? connector} (${staleReason})`, disabled: true }] : []),
  ];
  const showConnector = offered.length > 1 || (adapter === "pi" && offered.length > 0) || stale;
  const signsInWith = (stored && !stale ? stored : offered.length === 1 ? offered[0] : null)?.name ?? harness.name;

  function nameProblem(): string | null {
    const wanted = name.trim();
    if (!wanted) return "Give the agent a name.";
    if (takenNames.some((taken) => taken.toLowerCase() === wanted.toLowerCase()))
      return `An agent named "${wanted}" already exists.`;
    return null;
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    const problem = nameProblem();
    setNameError(problem);
    if (problem) return nameBox.current?.focus();
    setBusy(true);
    setFailure(null);
    const trimmed = name.trim();
    try {
      let id = agent?.id ?? null;
      if (agent) {
        await gql(
          `mutation ($id: UUID!, $name: String!, $adapter: String!, $model: String, $connector: String) {
             updateAgent(id: $id, name: $name, adapter: $adapter, model: $model, connector: $connector) { id }
           }`,
          { id: agent.id, name: trimmed, adapter, model, connector: connector || null },
        );
      } else {
        const data = await gql<{ createAgent: { id: string } }>(
          `mutation ($w: UUID!, $n: String!, $a: String!, $m: String, $c: String) {
             createAgent(workspaceId: $w, name: $n, adapter: $a, model: $m, connector: $c) { id }
           }`,
          { w: workspaceId, n: trimmed, a: adapter, m: model, c: connector || null },
        );
        id = data.createAgent.id;
      }
      if (id) {
        await setAgentInstructions(id, md);
        await setAgentSkills(id, picked);
      }
      await onSaved(id, agent ? `Saved "${trimmed}"` : `Created agent "${trimmed}"`);
    } catch (cause) {
      setFailure(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!agent) return;
    setBusy(true);
    try {
      await gql(`mutation ($id: UUID!) { deleteAgent(id: $id) }`, { id: agent.id });
      await onSaved(null, `Deleted "${agent.name}"`);
    } catch (cause) {
      setFailure(cause instanceof Error ? cause.message : String(cause));
      setConfirmDelete(false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialog}
      aria-labelledby="agent-dialog-title"
      className="settings-dialog agent-dialog"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="settings-form" noValidate onSubmit={(event) => void save(event)}>
        <div className="settings-head">
          <h2 id="agent-dialog-title">{agent ? "Edit agent" : "New agent"}</h2>
        </div>
        <div className="settings-content">
          <div className="field">
            <label className="field-label" htmlFor={ids.name}>
              Name <span aria-hidden="true">*</span>
            </label>
            <input
              id={ids.name}
              ref={nameBox}
              value={name}
              aria-invalid={nameError !== null}
              aria-describedby={nameError ? `${ids.name}-err` : undefined}
              onChange={(event) => {
                setName(event.target.value);
                setNameError(null);
              }}
            />
            {nameError ? (
              <p className="err" id={`${ids.name}-err`} role="alert">
                {nameError}
              </p>
            ) : null}
          </div>

          <div className="ag-two">
            <div className="field">
              <span className="field-label">Harness</span>
              <Select
                label="Harness"
                value={adapter}
                options={harnessOptions}
                onChange={(next) => {
                  setAdapter(next);
                  setModel("");
                  if (!connectors?.find((row) => row.id === connector)?.harnesses.includes(next)) setConnector("");
                }}
              />
            </div>
            <div className="field">
              <span className="field-label">Model</span>
              {models.length > 0 ? (
                <Select label="Model" value={model} options={modelOptions} onChange={chooseModel} disabled={!usable} searchable />
              ) : (
                <input
                  aria-label="Model"
                  value={model}
                  placeholder="provider/model"
                  disabled={!usable}
                  onChange={(event) => chooseModel(event.target.value)}
                />
              )}
            </div>
          </div>
          {usable ? (
            <p className="dim small">Runs on your {signsInWith}.</p>
          ) : (
            <p className="err">
              {harness.name} isn't connected. Connect it to use it.{" "}
              <button type="button" className="linkbtn" onClick={onOpenConnectors}>
                Open Connectors
              </button>
            </p>
          )}

          {showConnector ? (
            <div className="field">
              <span className="field-label">Signs in with</span>
              <Select label="Signs in with" value={connector} options={connectorOptions} onChange={setConnector} />
              <p className="dim small">
                {adapter === "pi"
                  ? "Lists the connectors for this model's provider and your Pi login. Automatic uses the provider's connector, then your Pi login."
                  : `${harness.name} can act on a plan login or a billed key. This chooses which.`}
              </p>
            </div>
          ) : null}

          <div className="field">
            <label className="field-label" htmlFor={ids.md}>
              AGENTS.md
            </label>
            <textarea
              id={ids.md}
              className="mono"
              rows={9}
              value={md}
              placeholder="Instructions this agent reads before every run."
              onChange={(event) => setMd(event.target.value)}
            />
            <p className="dim small">
              Read on every run, after the run contract and before the skills. This is where an agent that plans
              differs from one that implements.
            </p>
          </div>

          <div className="field">
            <div className="row spread">
              <span className="field-label">Skills</span>
              <button type="button" className="linkbtn" onClick={onOpenSkills}>
                Manage skills
              </button>
            </div>
            {skills.length === 0 ? (
              <p className="dim small">No skills yet.</p>
            ) : (
              <div className="tgl-group">
                {skills.map((skill) => {
                  const on = picked.includes(skill.id);
                  return (
                    <button
                      type="button"
                      key={skill.id}
                      className="tgl"
                      aria-pressed={on}
                      title={skill.description}
                      onClick={() => setPicked(on ? picked.filter((id) => id !== skill.id) : [...picked, skill.id])}
                    >
                      {skill.name}
                    </button>
                  );
                })}
              </div>
            )}
            <p className="dim small">
              AGENTS.md is read on every run. A skill is only loaded when its description fits the task. Skills are
              shared, so the same one can be on many agents.
            </p>
          </div>
          {failure ? (
            <p className="err" role="alert">
              {failure}
            </p>
          ) : null}
        </div>

        {confirmDelete && agent ? (
          <div className="settings-actions dlg-f" role="alert">
            <span className="dlg-confirm">Delete {agent.name}? Runs it started stay in the history.</span>
            <button type="button" className="ghost" onClick={() => setConfirmDelete(false)}>
              Cancel
            </button>
            <button type="button" className="danger" disabled={busy} onClick={() => void remove()}>
              Delete
            </button>
          </div>
        ) : (
          <div className="settings-actions dlg-f">
            {agent ? (
              <button type="button" className="danger dlg-del" disabled={busy} onClick={() => setConfirmDelete(true)}>
                Delete agent
              </button>
            ) : null}
            <button type="button" className="ghost" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className="primary" disabled={busy || !usable}>
              {agent ? "Save changes" : "Create agent"}
            </button>
          </div>
        )}
      </form>
    </dialog>
  );
}
