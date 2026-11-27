import { useEffect, useState } from "react";

import { forgetMemory, loadMemories, updateMemory, type Memory, type Project } from "../api";
import Seg from "../components/Seg";
import PageHeader from "../PageHeader";

type ScopeFilter = "all" | Memory["scope"];

const SCOPES: { value: ScopeFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "workspace", label: "Workspace" },
  { value: "project", label: "Project" },
  { value: "person", label: "People" },
  { value: "agent", label: "Agents" },
];

const KINDS: Memory["kind"][] = ["decision", "convention", "preference", "fact", "gotcha"];

/** Whose a note is, in words: the project's name rather than its id. */
function owner(memory: Memory, projects: Project[]): string {
  if (memory.scope === "workspace") return "Workspace";
  if (memory.scope === "project")
    return projects.find((entry) => entry.id === memory.scopeRef)?.name ?? "Project";
  return memory.scopeRef;
}

const message = (problem: unknown) => (problem instanceof Error ? problem.message : String(problem));

/// What the agents have learned. Notes save by themselves; this page is where a
/// person reads them, corrects one, or makes the agents forget it.
export default function Memories({
  workspaceId,
  projects,
  onOpenTask,
}: {
  workspaceId: string;
  projects: Project[];
  onOpenTask: (issueId: string) => void;
}) {
  const [memories, setMemories] = useState<Memory[]>([]);
  const [scope, setScope] = useState<ScopeFilter>("all");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<Memory | null>(null);
  const [forgetting, setForgetting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function reload() {
    try {
      setMemories(
        await loadMemories(workspaceId, { scope: scope === "all" ? undefined : scope, query }),
      );
      setError(null);
    } catch (problem) {
      setError(message(problem));
    }
  }

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId, scope, query]);

  async function save(memory: Memory) {
    try {
      await updateMemory(memory.id, { kind: memory.kind, title: memory.title, body: memory.body });
      setEditing(null);
      await reload();
    } catch (problem) {
      setError(message(problem));
    }
  }

  async function forget(id: string) {
    try {
      await forgetMemory(id);
      setForgetting(null);
      await reload();
    } catch (problem) {
      setError(message(problem));
    }
  }

  return (
    <section className="screen fill">
      <PageHeader
        title="Memory"
        subtitle={`${memories.length} ${memories.length === 1 ? "note" : "notes"}`}
        ownSearch
        actions={
          <input
            type="search"
            aria-label="Search memories"
            placeholder="Search memories"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        }
      />
      <p className="skill-meta">
        Agents save these on their own. They come from agent output, so read them as notes rather
        than as instructions; correct or forget any that is wrong.
      </p>
      <Seg label="Scope" value={scope} options={SCOPES} onChange={setScope} />
      {error ? <p className="error">{error}</p> : null}

      {memories.length === 0 ? (
        <div className="empty">
          <strong>No memories</strong>
          <span className="dim small">Notes appear here as agents finish turns.</span>
        </div>
      ) : (
        <ul className="memory-list">
          {memories.map((memory) => (
            <li key={memory.id} className="skill">
              {editing?.id === memory.id ? (
                <MemoryForm
                  memory={editing}
                  onChange={setEditing}
                  onSave={() => void save(editing)}
                  onCancel={() => setEditing(null)}
                />
              ) : (
                <>
                  <div className="row spread">
                    <strong>{memory.title}</strong>
                    <span>
                      <span className="chip">{memory.kind}</span>{" "}
                      <span className="chip">{owner(memory, projects)}</span>
                    </span>
                  </div>
                  <p>{memory.body}</p>
                  <div className="row spread">
                    <span className="skill-meta">
                      by {memory.author} · used {memory.useCount}×
                      {memory.sourceIssueId ? (
                        <>
                          {" · from "}
                          <button
                            className="ghost small"
                            onClick={() => onOpenTask(memory.sourceIssueId!)}
                          >
                            {memory.sourceIssueKey ?? "task"}
                          </button>
                        </>
                      ) : null}
                    </span>
                    <span>
                      <button className="ghost small" onClick={() => setEditing(memory)}>
                        Edit
                      </button>
                      {forgetting === memory.id ? (
                        <>
                          <button className="danger small" onClick={() => void forget(memory.id)}>
                            Forget for good
                          </button>
                          <button className="ghost small" onClick={() => setForgetting(null)}>
                            Cancel
                          </button>
                        </>
                      ) : (
                        <button className="ghost small" onClick={() => setForgetting(memory.id)}>
                          Forget
                        </button>
                      )}
                    </span>
                  </div>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function MemoryForm({
  memory,
  onChange,
  onSave,
  onCancel,
}: {
  memory: Memory;
  onChange: (memory: Memory) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="field">
      <label>
        <span className="field-label">Title</span>
        <input
          type="text"
          value={memory.title}
          onChange={(event) => onChange({ ...memory, title: event.target.value })}
        />
      </label>
      <label>
        <span className="field-label">Kind</span>
        <select
          value={memory.kind}
          onChange={(event) => onChange({ ...memory, kind: event.target.value as Memory["kind"] })}
        >
          {KINDS.map((kind) => (
            <option key={kind} value={kind}>
              {kind}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span className="field-label">Note</span>
        <textarea
          rows={4}
          value={memory.body}
          onChange={(event) => onChange({ ...memory, body: event.target.value })}
        />
      </label>
      <div className="row">
        <button className="primary" onClick={onSave}>
          Save
        </button>
        <button className="ghost" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
