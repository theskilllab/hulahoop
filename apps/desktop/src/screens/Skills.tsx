import { useEffect, useRef, useState } from "react";

import {
  createSkill,
  deleteSkill,
  loadAgentSkills,
  loadSkillUsage,
  loadSkills,
  setAgentSkills,
  updateSkill,
  type Skill,
} from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import RichText from "../RichText";

/** The file a new skill starts from, so the front matter is right without being
 *  remembered. The server reads the same two lines back out. */
const SKILL_TEMPLATE = `---
name: my-skill
description: What this skill does, and when an agent should use it.
---

# My skill

Write the steps the agent should follow.
`;

const NAME_RULE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

/** What a pasted `SKILL.md` says about itself, and what stops it being saved.
 *  The server applies the same rules; this copy exists so the dialog can say it
 *  while you type. */
function readSkillFile(text: string) {
  const issues: string[] = [];
  let name = "";
  let description = "";
  const front = /^---\r?\n([\s\S]*?)\r?\n---/.exec(text);
  if (!front) {
    issues.push("Start the file with a --- block that has a name and a description.");
  } else {
    for (const line of front[1].split("\n")) {
      const at = line.indexOf(":");
      if (at < 0) continue;
      const key = line.slice(0, at).trim();
      const value = line
        .slice(at + 1)
        .trim()
        .replace(/^["']|["']$/g, "");
      if (key === "name") name = value;
      else if (key === "description") description = value;
    }
  }
  if (front && !name) issues.push("Add a name: line.");
  else if (name && !NAME_RULE.test(name))
    issues.push("Write the name in lowercase words joined by hyphens, like tests-first.");
  if (front && !description)
    issues.push("Add a description: line. Agents read it to decide when to load the skill.");
  return { name, description, issues };
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

/// Skills are what an agent knows: written once, attached wherever it helps.
export default function Skills({ workspaceId }: { workspaceId: string }) {
  const [skills, setSkills] = useState<Skill[]>([]);
  // Who already carries what. A skill is worth writing once and attaching many
  // times, so the page has to show the attachments it already has.
  const [usage, setUsage] = useState<Record<string, string[]>>({});
  const [roster, setRoster] = useState<{ id: string; name: string; skills: Skill[] }[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const skill = skills.find((entry) => entry.id === selected) ?? null;
  const carried = (id: string) => usage[id] ?? [];
  const issues = skill ? readSkillFile(`---\nname: ${skill.name}\ndescription: ${skill.description}\n---\n`).issues : [];

  async function reload(select?: string) {
    try {
      const [loaded, who, carriers] = await Promise.all([
        loadSkills(workspaceId),
        loadSkillUsage(workspaceId),
        loadAgentSkills(workspaceId),
      ]);
      setSkills(loaded);
      setUsage(who);
      setRoster(carriers);
      if (select) setSelected(select);
      else if (!loaded.some((entry) => entry.id === selected)) setSelected(loaded[0]?.id ?? null);
      setError(null);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    }
  }

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId]);

  /// Attach or detach a skill from one agent. The whole set is sent because that
  /// is what the server replaces.
  async function toggle(agentId: string, skillId: string) {
    const agent = roster.find((entry) => entry.id === agentId);
    if (!agent) return;
    const carries = agent.skills.some((entry) => entry.id === skillId);
    const next = carries
      ? agent.skills.filter((entry) => entry.id !== skillId).map((entry) => entry.id)
      : [...agent.skills.map((entry) => entry.id), skillId];
    setBusy(true);
    try {
      await setAgentSkills(agentId, next);
      await reload();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  async function save(skill: Skill) {
    setBusy(true);
    try {
      await updateSkill(skill.id, {
        name: skill.name,
        description: skill.description,
        source: skill.source,
      });
      await reload(skill.id);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  async function remove(skill: Skill) {
    setBusy(true);
    try {
      await deleteSkill(skill.id);
      setConfirmDelete(null);
      setSelected(null);
      await reload();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  function edit(change: Partial<Skill>) {
    if (!skill) return;
    setSkills((all) => all.map((entry) => (entry.id === skill.id ? { ...entry, ...change } : entry)));
  }

  return (
    <section className="screen fill">
      <PageHeader
        title="Skills"
        subtitle={plural(skills.length, "skill")}
        actions={
          <button className="primary" onClick={() => setCreating(true)}>
            <Icon name="plus" size={13} />
            New skill
          </button>
        }
      />

      {error ? <p className="error">{error}</p> : null}

      <div className="skills-body">
        <div className="skills-roster">
          <p className="dim small">
            Skills are shared. Put one on as many agents as you like. AGENTS.md is read on every
            run, a skill is loaded only when its description fits the task.
          </p>
          {skills.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className="skill"
              aria-selected={selected === entry.id}
              onClick={() => {
                setSelected(entry.id);
                setConfirmDelete(null);
              }}
            >
              <strong className="truncate">{entry.name}</strong>
              <span className="clamp-2 dim small">
                {entry.description || "No description yet"}
              </span>
              <span className="dim small truncate">
                {carried(entry.id).length ? `Used by ${carried(entry.id).join(", ")}` : "No agent uses it yet"}
              </span>
            </button>
          ))}
        </div>

        {skill ? (
          <div className="skills-pane">
            <div>
              <h2>{skill.name}</h2>
              {skill.description ? <p className="dim small">{skill.description}</p> : null}
              {issues.length ? (
                <ul className="skill-issues" role="alert">
                  {issues.map((issue) => (
                    <li key={issue}>{issue}</li>
                  ))}
                </ul>
              ) : null}
            </div>

            <div className="field">
              <div className="row spread">
                <span className="field-label">Used by</span>
                <span className="dim small">
                  {carried(skill.id).length} of {roster.length} agents
                </span>
              </div>
              <div className="row">
                {roster.map((agent) => {
                  const carries = agent.skills.some((picked) => picked.id === skill.id);
                  return (
                    <button
                      key={agent.id}
                      type="button"
                      className="tgl"
                      aria-pressed={carries}
                      disabled={busy}
                      onClick={() => void toggle(agent.id, skill.id)}
                    >
                      {agent.name}
                    </button>
                  );
                })}
              </div>
              <p className="dim small">
                Edit the text once and every agent that uses this skill gets the change.
              </p>
            </div>

            {skill.builtin ? (
              <div className="field">
                <span className="field-label">The contract every turn reads</span>
                <p className="dim small">
                  The server writes this text and reads the markers in it, so it is shown here and
                  never edited.
                </p>
                <RichText
                  value={skill.source}
                  editable={false}
                  minHeight={320}
                  ariaLabel={`${skill.name} body`}
                  onChange={() => {}}
                />
              </div>
            ) : (
              <>
                <div className="field">
                  <span className="field-label">Name</span>
                  <input value={skill.name} onChange={(event) => edit({ name: event.target.value })} />
                </div>
                <div className="field">
                  <span className="field-label">When is it loaded</span>
                  <input
                    placeholder="one line: the task it fits"
                    value={skill.description}
                    onChange={(event) => edit({ description: event.target.value })}
                  />
                </div>
                <div className="field">
                  <span className="field-label">SKILL.md</span>
                  <RichText
                    value={skill.source}
                    ariaLabel={`${skill.name} body`}
                    placeholder="The knowledge itself, in markdown."
                    minHeight={320}
                    onChange={(source) => edit({ source })}
                  />
                </div>
                <div className="row">
                  <button className="primary" disabled={busy || issues.length > 0} onClick={() => void save(skill)}>
                    Save
                  </button>
                  {confirmDelete === skill.id ? null : (
                    <button className="ghost" disabled={busy} onClick={() => setConfirmDelete(skill.id)}>
                      Delete skill
                    </button>
                  )}
                </div>
                {confirmDelete === skill.id ? (
                  <div className="review-bar" role="alert">
                    <span className="od-fill">
                      <strong>Delete “{skill.name}”?</strong>{" "}
                      {carried(skill.id).length
                        ? `${plural(carried(skill.id).length, "agent")} will lose it.`
                        : "No agent uses it."}
                    </span>
                    <button className="ghost" onClick={() => setConfirmDelete(null)}>
                      Cancel
                    </button>
                    <button className="danger" disabled={busy} onClick={() => void remove(skill)}>
                      Delete
                    </button>
                  </div>
                ) : null}
              </>
            )}
          </div>
        ) : (
          <div className="skills-pane">
            <div className="empty">
              <strong>No skills yet</strong>
              <span className="dim small">Create one to share a way of working between agents.</span>
            </div>
          </div>
        )}
      </div>

      {creating ? (
        <NewSkill
          skills={skills}
          roster={roster}
          workspaceId={workspaceId}
          onClose={() => setCreating(false)}
          onCreated={async (id) => {
            setCreating(false);
            await reload(id);
          }}
        />
      ) : null}
    </section>
  );
}

/** A skill starts as a `SKILL.md`, because that is the file an agent reads. The
 *  name and description are in it, so the dialog is one textarea and a live
 *  reading of what those lines say. */
function NewSkill({
  workspaceId,
  skills,
  roster,
  onClose,
  onCreated,
}: {
  workspaceId: string;
  skills: Skill[];
  roster: { id: string; name: string; skills: Skill[] }[];
  onClose: () => void;
  onCreated: (id: string) => Promise<void>;
}) {
  const [source, setSource] = useState(SKILL_TEMPLATE);
  const [agents, setAgents] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const field = useRef<HTMLTextAreaElement | null>(null);

  const read = readSkillFile(source);
  const taken = read.name ? skills.some((entry) => entry.name === read.name) : false;
  const issues = taken ? [...read.issues, "Another skill already uses this name."] : read.issues;

  useEffect(() => {
    field.current?.setSelectionRange(
      SKILL_TEMPLATE.indexOf("my-skill"),
      SKILL_TEMPLATE.indexOf("my-skill") + "my-skill".length,
    );
  }, []);

  async function create() {
    setBusy(true);
    try {
      const created = await createSkill(workspaceId, read.name, read.description, source);
      // Attaching at creation is part of writing it: a skill nobody carries was
      // written for nothing.
      for (const agentId of agents) {
        const agent = roster.find((entry) => entry.id === agentId);
        if (!agent) continue;
        await setAgentSkills(agentId, [...agent.skills.map((entry) => entry.id), created.id]);
      }
      await onCreated(created.id);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
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
          <h2>New skill</h2>
          <button className="ghost" onClick={onClose} aria-label="Close">
            <Icon name="close" size={14} />
          </button>
        </header>

        <div className="field">
          <span className="field-label">SKILL.md</span>
          <textarea
            ref={field}
            className="mono skill-file"
            rows={12}
            spellCheck={false}
            value={source}
            autoFocus
            onChange={(event) => setSource(event.target.value)}
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault();
              const file = event.dataTransfer.files[0];
              if (file) void file.text().then(setSource);
            }}
          />
          <p className="dim small">Drop a skill file here to read it instead of typing one.</p>
        </div>

        {issues.length ? (
          <ul className="skill-issues" role="alert">
            {issues.map((issue) => (
              <li key={issue}>{issue}</li>
            ))}
          </ul>
        ) : (
          <span className="chip ok">
            <Icon name="check" size={12} />
            {read.name}
          </span>
        )}

        <div className="field">
          <span className="field-label">Give it to</span>
          <div className="row">
            {roster.map((agent) => {
              const on = agents.includes(agent.id);
              return (
                <button
                  key={agent.id}
                  type="button"
                  className="tgl"
                  aria-pressed={on}
                  onClick={() =>
                    setAgents(on ? agents.filter((id) => id !== agent.id) : [...agents, agent.id])
                  }
                >
                  {agent.name}
                </button>
              );
            })}
          </div>
        </div>

        {error ? <p className="error">{error}</p> : null}

        <div className="row">
          <button className="primary" disabled={busy || issues.length > 0} onClick={() => void create()}>
            Create skill
          </button>
          <button className="ghost" disabled={busy} onClick={onClose}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
