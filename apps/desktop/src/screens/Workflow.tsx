import { useEffect, useMemo, useState } from "react";

import {
  BOARD_STATUSES,
  loadWorkflow,
  saveWorkflow,
  type Agent,
  type IssueStatus,
  type WorkflowDoc,
  type WorkflowSection,
} from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";

const OWNERS = ["human", "agent"] as const;

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
        `When: ${section.when ?? ""}`,
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
  const vertical = Math.abs(a.x - b.x) < 40;
  if (vertical) {
    const bow = 96;
    return `M ${a.x + W / 2} ${a.y} C ${a.x + W / 2 + bow} ${a.y}, ${b.x + W / 2 + bow} ${b.y}, ${b.x + W / 2} ${b.y}`;
  }
  const right = b.x > a.x;
  const x1 = a.x + (right ? W / 2 : -W / 2);
  const x2 = b.x + (right ? -W / 2 : W / 2);
  const mid = (x1 + x2) / 2;
  return `M ${x1} ${a.y} C ${mid} ${a.y}, ${mid} ${b.y}, ${x2} ${b.y}`;
}

function statusOf(section: WorkflowSection): string {
  return section.status;
}

function Diagram({
  sections,
  errors,
}: {
  sections: WorkflowSection[];
  errors: string[];
}) {
  const known = new Set(Object.keys(NODE));
  const edges = sections
    .filter((section) => section.next && known.has(section.next) && known.has(section.status))
    .map((section) => ({ from: statusOf(section), to: section.next as string, label: section.when ?? "then" }));

  return (
    <div className="diagram-block">
      <div className="diagram-label">State diagram</div>
      <div className="diagram" role="img" aria-label="the workflow as a diagram">
        <svg viewBox="0 0 1180 360" preserveAspectRatio="xMinYMin meet">
          <defs>
            <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto">
              <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--muted)" />
            </marker>
          </defs>
          {edges.map((edge) => {
            const a = centre(edge.from);
            const b = centre(edge.to);
            return (
              <g key={`${edge.from}-${edge.to}`}>
                <path d={edgePath(edge.from, edge.to)} className="diagram-edge" markerEnd="url(#arrow)" />
                <text x={(a.x + b.x) / 2} y={(a.y + b.y) / 2 - 8} className="diagram-edge-label">
                  {edge.label.length > 34 ? `${edge.label.slice(0, 33)}…` : edge.label}
                </text>
              </g>
            );
          })}
        </svg>
        {sections
          .filter((section) => known.has(section.status))
          .map((section) => {
            const node = NODE[section.status];
            const agent = section.owner === "agent";
            return (
              <div
                key={section.status}
                className={`dnode ${agent ? "agent" : ""} ${section.next ? "" : "final"}`}
                style={{ left: node.x, top: node.y }}
                title={`${section.title} — ${section.owner}`}
              >
                <strong>{section.title}</strong>
                <small>
                  {agent ? (section.agent ? section.agent : "Any idle agent") : "You"}
                </small>
              </div>
            );
          })}
      </div>
      <p className="diagram-hint">
        Not what you meant? Switch to Markdown and correct the lines, or use the fields under each
        status below.
      </p>
      <div className="legend">
        <span>
          <i className="sw agent" /> An agent handles it
        </span>
        <span>
          <i className="sw human" /> You handle it
        </span>
        <span>
          <i className="sw final" /> Final status
        </span>
      </div>

      <div className="diagram-label">Checks</div>
      {errors.length === 0 ? (
        <p className="check-ok">
          <Icon name="check" size={13} /> No problems found
        </p>
      ) : (
        <ul className="problems">
          {errors.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      )}

      <div className="diagram-label">Transitions</div>
      <table className="transitions">
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
              <td>{section.owner === "agent" ? (section.agent ?? "Any idle agent") : "You"}</td>
              <td className="dim">{section.when ?? "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/// The workflow is one markdown document per workspace. This screen shows it three
/// ways: the text, the diagram the text produces, or both side by side.
export default function WorkflowScreen({
  workspaceId,
  agents,
}: {
  workspaceId: string;
  agents: Agent[];
}) {
  const [doc, setDoc] = useState<WorkflowDoc | null>(null);
  const [source, setSource] = useState("");
  const [sections, setSections] = useState<WorkflowSection[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tab, setTab] = useState<"both" | "markdown" | "diagram">("both");

  useEffect(() => {
    let live = true;
    loadWorkflow(workspaceId)
      .then((next) => {
        if (!live) return;
        setDoc(next);
        setSource(next.source);
        setSections(next.sections);
      })
      .catch((problem) => live && setError(problem.message));
    return () => {
      live = false;
    };
  }, [workspaceId]);

  const dirty = doc !== null && source !== doc.source;

  function change(next: WorkflowSection[]) {
    setSections(next);
    setSource(rewrite(source, next));
  }

  async function save() {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const next = await saveWorkflow(workspaceId, source);
      setDoc(next);
      setSections(next.sections);
      setNote("saved — the board follows this document");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
    } finally {
      setBusy(false);
    }
  }

  const flow = useMemo(
    () => BOARD_STATUSES.map((status) => sections.find((section) => section.status === status)),
    [sections],
  );

  return (
    <section className="screen workflow">
      <PageHeader
        title="Workflow"
        subtitle="One document per workspace. Every ## status heading is a column and says who works it, which agent picks it up, and where it leads."
        actions={
          <button className="primary" onClick={save} disabled={busy || !dirty}>
            {busy ? "Saving…" : dirty ? "Save workflow" : "Saved"}
          </button>
        }
      />

      {error ? <p className="error">{error}</p> : null}
      {note ? <p className="note">{note}</p> : null}
      {doc && doc.errors.length > 0 ? (
        <ul className="problems">
          {doc.errors.map((problem) => (
            <li key={problem}>{problem}</li>
          ))}
        </ul>
      ) : null}

      <div className="tabs workflow-tabs" role="tablist">
        {(["both", "markdown", "diagram"] as const).map((name) => (
          <button
            key={name}
            role="tab"
            aria-selected={tab === name}
            className={tab === name ? "tab on" : "tab"}
            onClick={() => setTab(name)}
          >
            {name === "both" ? "Both" : name === "markdown" ? "Markdown" : "Diagram"}
          </button>
        ))}
      </div>

      <div className={`workflow-body ${tab}`}>
        {tab !== "diagram" ? (
          <label className="workflow-editor">
            <span className="label">WORKFLOWS.md</span>
            <textarea
              spellCheck={false}
              value={source}
              onChange={(event) => setSource(event.target.value)}
              rows={28}
            />
          </label>
        ) : null}

        {tab !== "markdown" ? (
          <Diagram sections={sections} errors={doc?.errors ?? []} />
        ) : (
          <div className="workflow-preview">
            <div className="label">what the server reads</div>
            <table>
              <thead>
                <tr>
                  <th>column</th>
                  <th>owner</th>
                  <th>agent</th>
                  <th>next</th>
                </tr>
              </thead>
              <tbody>
                {sections.map((section) => (
                  <tr key={section.status}>
                    <td>{section.title}</td>
                    <td>
                      <span className={`chip ${section.owner}`}>{section.owner}</span>
                    </td>
                    <td className="mono">{section.agent ?? "—"}</td>
                    <td className="mono">{section.next ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      <details className="workflow-fields">
        <summary>Edit a status</summary>
        <div className="status-cards">
          {flow.map((section) =>
            section ? (
              <article key={section.status} className="flow-node">
                <div className="spread">
                  <span className="name">{section.title}</span>
                  <span className={`chip ${section.owner}`}>{section.owner}</span>
                </div>
                <div className="owner" role="group" aria-label={`who works ${section.title}`}>
                  {OWNERS.map((owner) => (
                    <button
                      key={owner}
                      className={section.owner === owner ? "on" : ""}
                      onClick={() =>
                        change(
                          sections.map((entry) =>
                            entry.status === section.status ? { ...entry, owner } : entry,
                          ),
                        )
                      }
                    >
                      {owner}
                    </button>
                  ))}
                </div>
                {section.owner === "agent" ? (
                  <select
                    value={section.agent ?? ""}
                    onChange={(event) =>
                      change(
                        sections.map((entry) =>
                          entry.status === section.status
                            ? { ...entry, agent: event.target.value || null }
                            : entry,
                        ),
                      )
                    }
                  >
                    <option value="">any agent on the project</option>
                    {agents.map((agent) => (
                      <option key={agent.id} value={agent.name}>
                        {agent.name}
                      </option>
                    ))}
                  </select>
                ) : null}
                <label className="stack">
                  <span className="label">then</span>
                  <select
                    value={section.next ?? ""}
                    onChange={(event) =>
                      change(
                        sections.map((entry) =>
                          entry.status === section.status
                            ? { ...entry, next: (event.target.value || null) as IssueStatus | null }
                            : entry,
                        ),
                      )
                    }
                  >
                    <option value="">stays here</option>
                    {BOARD_STATUSES.filter((status) => status !== section.status).map((status) => (
                      <option key={status} value={status}>
                        {sections.find((entry) => entry.status === status)?.title ?? status}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="stack">
                  <span className="label">when</span>
                  <input
                    value={section.when ?? ""}
                    placeholder="the sentence the arrow shows"
                    onChange={(event) =>
                      change(
                        sections.map((entry) =>
                          entry.status === section.status ? { ...entry, when: event.target.value } : entry,
                        ),
                      )
                    }
                  />
                </label>
              </article>
            ) : null,
          )}
        </div>
      </details>
    </section>
  );
}
