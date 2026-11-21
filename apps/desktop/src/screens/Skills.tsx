import { useEffect, useState } from "react";

import { createSkill, deleteSkill, loadSkills, updateSkill, type Skill } from "../api";

/// Skills are what an agent knows: written once, attached wherever it helps.
export default function Skills({ workspaceId }: { workspaceId: string }) {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [openId, setOpenId] = useState<string | null>(null);
  const [draft, setDraft] = useState({ name: "", description: "", source: "" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function reload() {
    try {
      setSkills(await loadSkills(workspaceId));
      setError(null);
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    }
  }

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId]);

  async function add() {
    setBusy(true);
    try {
      await createSkill(workspaceId, draft.name, draft.description, draft.source);
      setDraft({ name: "", description: "", source: "" });
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
      setOpenId(null);
      await reload();
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
      await reload();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="screen skills">
      <header>
        <div>
          <h1>Skills</h1>
          <p className="dim">
            What an agent knows. One skill is one piece of knowledge — a convention, a checklist, a
            house rule with the story behind it — and an agent carries the ones it needs.
          </p>
        </div>
      </header>

      {error ? <p className="error">{error}</p> : null}

      <div className="skills-grid">
        <div className="skills-list">
          {skills.length === 0 ? (
            <p className="dim">No skills yet. Write the first one on the right.</p>
          ) : null}
          {skills.map((skill) => (
            <article key={skill.id} className="skill">
              <header>
                <h2>{skill.name}</h2>
                <button className="link" onClick={() => setOpenId(openId === skill.id ? null : skill.id)}>
                  {openId === skill.id ? "close" : "edit"}
                </button>
              </header>
              {skill.description ? <p className="dim">{skill.description}</p> : null}
              {openId === skill.id ? (
                <div className="stack">
                  <input
                    value={skill.name}
                    onChange={(event) =>
                      setSkills((all) =>
                        all.map((entry) => (entry.id === skill.id ? { ...entry, name: event.target.value } : entry)),
                      )
                    }
                  />
                  <input
                    value={skill.description}
                    placeholder="one line: when do you want this loaded?"
                    onChange={(event) =>
                      setSkills((all) =>
                        all.map((entry) =>
                          entry.id === skill.id ? { ...entry, description: event.target.value } : entry,
                        ),
                      )
                    }
                  />
                  <textarea
                    rows={12}
                    value={skill.source}
                    onChange={(event) =>
                      setSkills((all) =>
                        all.map((entry) =>
                          entry.id === skill.id ? { ...entry, source: event.target.value } : entry,
                        ),
                      )
                    }
                  />
                  <div className="row">
                    <button className="primary" disabled={busy} onClick={() => save(skill)}>
                      Save
                    </button>
                    <button className="ghost" disabled={busy} onClick={() => remove(skill)}>
                      Delete
                    </button>
                  </div>
                </div>
              ) : (
                <pre className="skill-body">{skill.source}</pre>
              )}
            </article>
          ))}
        </div>

        <aside className="skill-new">
          <div className="label">new skill</div>
          <input
            placeholder="name — e.g. house style"
            value={draft.name}
            onChange={(event) => setDraft({ ...draft, name: event.target.value })}
          />
          <input
            placeholder="when is it useful?"
            value={draft.description}
            onChange={(event) => setDraft({ ...draft, description: event.target.value })}
          />
          <textarea
            rows={12}
            placeholder="the knowledge itself, in markdown"
            value={draft.source}
            onChange={(event) => setDraft({ ...draft, source: event.target.value })}
          />
          <button className="primary" disabled={busy || !draft.name.trim()} onClick={add}>
            Create skill
          </button>
        </aside>
      </div>
    </section>
  );
}
