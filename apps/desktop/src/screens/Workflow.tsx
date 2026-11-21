import { useEffect, useMemo, useRef, useState } from "react";

import {
  loadWorkflow,
  saveWorkflow,
  type Agent,
  type WorkflowDoc,
  type WorkflowSection,
} from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";

type Tab = "both" | "markdown" | "diagram";

/// Rewrite the status sections of a workflow document, keeping whatever preamble
/// the person wrote above the first `## ` heading. The diagram and the document
/// are the same thing, so editing one has to move the other.
function rewrite(source: string, sections: WorkflowSection[]): string {
  const firstHeading = source.search(/^##\s/m);
  const preamble = firstHeading === -1 ? source.trimEnd() : source.slice(0, firstHeading).trimEnd();
  const body = sections
    .map((section) =>
      [
        `## ${section.title}`,
        `Owner: ${section.owner}`,
        `Agent: ${section.agent ?? "any"}`,
        `Next: ${section.next ?? "none"}`,
        section.when ? `When: ${section.when}` : "",
        section.body.trim(),
      ]
        .filter((line) => line.length > 0)
        .join("\n"),
    )
    .join("\n\n");
  return `${preamble}\n\n${body}\n`;
}

/// Where each status sits on the canvas. Fixed, because a diagram that rearranges
/// itself every time a document changes is a diagram nobody can read twice.
const NODE: Record<string, { x: number; y: number }> = {
  backlog: { x: 40, y: 168 },
  todo: { x: 268, y: 168 },
  in_progress: { x: 500, y: 64 },
  blocked: { x: 500, y: 268 },
  in_review: { x: 736, y: 168 },
  done: { x: 972, y: 64 },
};

const W = 150;
const H = 62;

function centre(status: string): { x: number; y: number } {
  const node = NODE[status] ?? { x: 40, y: 168 };
  return { x: node.x + W / 2, y: node.y + H / 2 };
}

/// A cubic that leaves the right edge of one node and enters the left edge of the
/// next, except between the two stacked nodes, where it bows sideways instead.
function edgePath(from: string, to: string): string {
  const a = centre(from);
  const b = centre(to);
  const stacked = Math.abs(b.x - a.x) < 1;
  if (stacked) {
    const bow = b.y > a.y ? 1 : -1;
    return `M ${a.x} ${a.y} C ${a.x + 78 * bow} ${a.y}, ${b.x + 78 * bow} ${b.y}, ${b.x} ${b.y}`;
  }
  const mid = (a.x + b.x) / 2;
  return `M ${a.x} ${a.y} C ${mid} ${a.y}, ${mid} ${b.y}, ${b.x} ${b.y}`;
}

function Diagram({
  sections,
  editing,
  onEdit,
}: {
  sections: WorkflowSection[];
  editing: string | null;
  onEdit: (id: string) => void;
}) {
  const edges = sections
    .filter((section) => section.next && NODE[section.next])
    .map((section) => ({
      id: section.status,
      from: section.status,
      to: section.next as string,
      label: section.when ?? "then",
    }));
  return (
    <div className="wf-canvas">
      <svg viewBox="0 0 1180 360" role="img" aria-label="Workflow diagram">
        <defs>
          <marker id="wf-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M0 0 L8 4 L0 8 z" fill="var(--line-strong)" />
          </marker>
        </defs>
        {edges.map((edge) => {
          const a = centre(edge.from);
          const b = centre(edge.to);
          const stacked = Math.abs(b.x - a.x) < 1;
          const bow = b.y > a.y ? 1 : -1;
          const mid = stacked
            ? { x: a.x + 78 * bow, y: (a.y + b.y) / 2 }
            : { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
          return (
            <g key={`e-${edge.id}`} className="wf-edge" onClick={() => onEdit(edge.id)}>
              <path d={edgePath(edge.from, edge.to)} markerEnd="url(#wf-arrow)" />
              <text x={mid.x} y={mid.y - 7} textAnchor="middle">
                {edge.label.length > 26 ? `${edge.label.slice(0, 26)}…` : edge.label}
              </text>
            </g>
          );
        })}
      </svg>
      {sections.map((section) => {
        const node = NODE[section.status];
        if (!node) return null;
        const agent = section.owner === "agent";
        return (
          <button
            key={section.status}
            className={`wf-node ${agent ? "agent" : "human"} ${section.next ? "" : "final"} ${
              editing === section.status ? "on" : ""
            }`}
            style={{ left: node.x, top: node.y, width: W, height: H }}
            onClick={() => onEdit(section.status)}
            title={`${section.title} — ${agent ? (section.agent ?? "any agent") : "you"}`}
          >
            <span className="n-name">{section.title}</span>
            <span className="n-who">{agent ? (section.agent ?? "no agent yet") : "You"}</span>
          </button>
        );
      })}
    </div>
  );
}

export default function Workflow({
  workspaceId,
  agents,
  onSaved,
}: {
  workspaceId: string;
  agents: Agent[];
  onSaved?: () => void;
}) {
  const [doc, setDoc] = useState<WorkflowDoc | null>(null);
  const [source, setSource] = useState("");
  const [tab, setTab] = useState<Tab>("both");
  const [editing, setEditing] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const [mentions, setMentions] = useState<{ at: number; items: Agent[] } | null>(null);
  const area = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    loadWorkflow(workspaceId).then((next) => {
      setDoc(next);
      setSource(next?.source ?? "");
    });
  }, [workspaceId]);

  const dirty = doc !== null && source !== doc.source;
  const known = useMemo(() => new Set(agents.map((agent) => agent.name.toLowerCase())), [agents]);
  const unknown = useMemo(
    () =>
      (doc?.sections ?? []).filter(
        (section) => section.owner === "agent" && section.agent && !known.has(section.agent.toLowerCase()),
      ),
    [doc, known],
  );
  const sections = doc?.sections ?? [];
  const edited = useMemo(() => sections.find((entry) => entry.status === editing) ?? null, [sections, editing]);

  function change(patch: (next: WorkflowSection[]) => WorkflowSection[]) {
    if (!doc) return;
    const next = patch(doc.sections);
    setDoc({ ...doc, sections: next });
    setSource(rewrite(source, next));
  }

  async function save() {
    setNote("");
    const saved = await saveWorkflow(workspaceId, source).catch((error: Error) => {
      setNote(error.message);
      return null;
    });
    if (!saved) return;
    setDoc(saved);
    setSource(saved.source);
    setEditing(null);
    setNote(saved.errors.length ? `saved, ${saved.errors.length} check(s)` : "saved");
    onSaved?.();
  }

  /// An `@` name in the document is how a workflow points at a real agent.
  function onType(value: string, caret: number) {
    setSource(value);
    const before = value.slice(0, caret);
    const match = /(^|[\s(])@([\w-]*)$/.exec(before);
    const found = match
      ? agents.filter((agent) => agent.name.toLowerCase().split(" ").some((word) => word.startsWith(match[2].toLowerCase())))
      : [];
    setMentions(found.length ? { at: caret - match![2].length - 1, items: found } : null);
  }

  function pick(agent: Agent) {
    if (!mentions) return;
    const caret = area.current?.selectionStart ?? source.length;
    const next = `${source.slice(0, mentions.at)}@${agent.name.replace(/\s+/g, "")}${source.slice(caret)}`;
    setSource(rewrite(next, sections));
    setMentions(null);
    area.current?.focus();
  }

  const tabs = (
    <div className="seg small">
      {(["both", "markdown", "diagram"] as Tab[]).map((value) => (
        <button key={value} className={tab === value ? "on" : ""} onClick={() => setTab(value)}>
          {value === "both" ? "Both" : value === "markdown" ? "Markdown" : "Diagram"}
        </button>
      ))}
    </div>
  );

  return (
    <div className="screen">
      <PageHeader
        title="Workflow"
        subtitle="Workflow for this workspace"
        actions={
          <>
            {tabs}
            <button className="btn primary" onClick={() => void save()} disabled={!dirty}>
              Save
            </button>
          </>
        }
      />

      {note ? <p className="dim small">{note}</p> : null}
      {unknown.length ? (
        <ul className="problems">
          {unknown.map((section) => (
            <li key={section.status}>
              `{section.title}` names an agent nobody has: `{section.agent}`.
            </li>
          ))}
        </ul>
      ) : null}
      {doc && doc.errors.length ? (
        <ul className="problems">
          {doc.errors.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      ) : (
        <p className="wf-ok">
          <span className="chip ok">
            <Icon name="check" size={12} /> No problems found
          </span>
          <span className="dim small">The document and the statuses agree.</span>
        </p>
      )}

      <div className={`wf ${tab}`}>
        {tab !== "diagram" ? (
          <section className="wf-src">
            <div className="head">
              <span className="mono dim small">WORKFLOWS.md</span>
              {dirty ? <span className="chip warn small">Unsaved</span> : null}
              <button className="link" style={{ marginLeft: "auto" }} onClick={() => setSource(doc?.source ?? "")} disabled={!dirty}>
                Revert
              </button>
            </div>
            <textarea
              ref={area}
              value={source}
              spellCheck={false}
              onChange={(event) => onType(event.target.value, event.target.selectionStart)}
              onKeyDown={(event) => {
                if (!mentions) return;
                if (event.key === "Enter" || event.key === "Tab") {
                  event.preventDefault();
                  pick(mentions.items[0]);
                }
                if (event.key === "Escape") setMentions(null);
              }}
            />
            {mentions ? (
              <div className="mention-pop">
                {mentions.items.map((agent) => (
                  <button key={agent.id} onMouseDown={(event) => event.preventDefault()} onClick={() => pick(agent)}>
                    {agent.name}
                  </button>
                ))}
              </div>
            ) : null}
          </section>
        ) : null}

        {tab !== "markdown" ? (
          <section className="wf-diagram">
            <div className="wf-guide">
              {edited ? (
                <>
                  <strong>{edited.title}</strong>
                  <div className="seg small">
                    {(["human", "agent"] as const).map((owner) => (
                      <button
                        key={owner}
                        className={edited.owner === owner ? "on" : ""}
                        onClick={() =>
                          change((next) => next.map((s) => (s.status === edited.status ? { ...s, owner } : s)))
                        }
                      >
                        {owner === "human" ? "I handle it" : "An agent handles it"}
                      </button>
                    ))}
                  </div>
                  {edited.owner === "agent" ? (
                    <select
                      className="select small"
                      value={edited.agent ?? ""}
                      onChange={(event) =>
                        change((next) =>
                          next.map((s) =>
                            s.status === edited.status ? { ...s, agent: event.target.value || null } : s,
                          ),
                        )
                      }
                    >
                      <option value="" disabled>
                        pick an agent
                      </option>
                      {agents.map((agent) => (
                        <option key={agent.id} value={agent.name}>
                          {agent.name}
                        </option>
                      ))}
                    </select>
                  ) : null}
                  <input
                    className="input small"
                    placeholder="the condition, in words"
                    value={edited.when ?? ""}
                    onChange={(event) =>
                      change((next) =>
                        next.map((s) => (s.status === edited.status ? { ...s, when: event.target.value } : s)),
                      )
                    }
                  />
                  <button className="link" onClick={() => setEditing(null)}>
                    Done
                  </button>
                </>
              ) : (
                <span className="dim small">
                  Click a status or an arrow to change who works it, or the sentence that moves it along.
                </span>
              )}
            </div>

            <Diagram sections={sections} editing={editing} onEdit={setEditing} />

            <div className="wf-legend">
              <span className="sw agent" /> An agent handles it
              <span className="sw human" /> You handle it
              <span className="sw final" /> Final status
            </div>

            <table className="vals">
              <thead>
                <tr>
                  <th>From</th>
                  <th>To</th>
                  <th>Handled by</th>
                  <th>When</th>
                </tr>
              </thead>
              <tbody>
                {sections.map((section) => (
                  <tr key={section.status}>
                    <td>{section.title}</td>
                    <td>{sections.find((entry) => entry.status === section.next)?.title ?? "—"}</td>
                    <td>
                      {section.owner === "agent" ? (section.agent ?? "Any idle agent") : "You"}
                    </td>
                    <td className="dim">{section.when ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        ) : null}
      </div>
    </div>
  );
}
