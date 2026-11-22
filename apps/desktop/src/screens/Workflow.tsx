import { useEffect, useMemo, useRef, useState } from "react";

import { loadWorkflow, saveWorkflow, type Agent, type WorkflowDoc, type WorkflowSection } from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";

/**
 * WORKFLOWS.md, as the prototype draws it: a file pane on the left, and on the right
 * an inspector for whatever is selected, the state diagram, the checks and the
 * transitions table. The source of truth is the text; everything on the right is a
 * reading of it, and every edit is written back into its own section's lines.
 */

type View = "both" | "md" | "diagram";

/** One status as this screen edits it, before it is written back into the text. */
interface Editable {
  owner: "human" | "agent";
  agents: string[];
  any: boolean;
  moves: { to: string; when: string }[];
}

const KEYWORDS = /^(owner|user|users|agent|agents|next|when)\s*:/i;

function sectionOf(sections: WorkflowSection[], title: string): WorkflowSection | undefined {
  return sections.find((section) => section.title === title);
}

/** The metadata lines of the section whose heading is `title`, as line indexes. */
function block(source: string, title: string): { heading: number; start: number; end: number } | null {
  const lines = source.split("\n");
  const heading = lines.findIndex(
    (line) => /^##\s+(.*)$/.exec(line)?.[1]?.trim() === title,
  );
  if (heading === -1) return null;
  let end = heading + 1;
  while (end < lines.length && KEYWORDS.test(lines[end].trim())) end += 1;
  return { heading, start: heading + 1, end };
}

/** What the owner lines for one section look like: one for people, one for agents. */
function ownerLines(state: Editable): string[] {
  const agents = state.any ? ["any"] : state.agents;
  if (state.owner === "human" || agents.length === 0) return ["Owner: human"];
  // "Agent:" reads as the prototype writes it: one line, names separated by commas,
  // the first name being the one that picks the status up.
  return [`Agent: ${agents.map((name) => (name === "any" ? "any" : `@${name}`)).join(", ")}`];
}

function nextLines(state: Editable): string[] {
  if (state.moves.length === 0) return ["Next: none"];
  return state.moves.map((move) => `Next: ${move.to}${move.when ? ` - ${move.when}` : ""}`);
}

/** Replace one section's metadata, leaving its body and every other line alone. */
function apply(source: string, title: string, state: Editable): string {
  const spot = block(source, title);
  if (!spot) return source;
  const lines = source.split("\n");
  const written = [...ownerLines(state), ...nextLines(state)];
  return [...lines.slice(0, spot.start), ...written, ...lines.slice(spot.end)].join("\n");
}

function editable(section: WorkflowSection): Editable {
  const any = section.agents.some((name) => name.toLowerCase() === "any");
  return {
    owner: section.owner === "agent" ? "agent" : "human",
    agents: section.agents.filter((name) => name.toLowerCase() !== "any"),
    any: any || (section.owner === "agent" && section.agents.length === 0),
    moves: (section.transitions ?? []).map((move) => ({
      to: sectionOfTitle(move.title),
      when: move.when ?? "",
    })),
  };
}

/** The server names a move by its target's title; the document names it the same way. */
function sectionOfTitle(title: string): string {
  return title;
}

/* ------------------------------------------------------------------ diagram --- */

const W = 168;
const H = 58;
const GX = 128;
const GY = 28;
const PAD = 20;

interface Node {
  key: string;
  title: string;
  agents: string[];
  owner: string;
  x: number;
  y: number;
  rank: number;
}

interface Edge {
  from: string;
  to: string;
  when: string | null;
  back: boolean;
  path: string;
  label: { x: number; y: number; text: string } | null;
}

function layout(sections: WorkflowSection[]): { nodes: Node[]; edges: Edge[]; width: number; height: number } {
  const index = new Map(sections.map((section, i) => [section.title, i]));
  const raw = sections.map((section) =>
    (section.transitions ?? [])
      .map((move) => ({ a: index.get(section.title)!, b: index.get(move.title), when: move.when }))
      .filter((edge) => edge.b !== undefined),
  );
  const rank = sections.map(() => -1);
  if (sections.length > 0) rank[0] = 0;
  const queue = sections.length > 0 ? [0] : [];
  while (queue.length > 0) {
    const i = queue.shift()!;
    for (const edge of raw[i]) {
      if (rank[edge.b!] < 0) {
        rank[edge.b!] = rank[i] + 1;
        queue.push(edge.b!);
      }
    }
  }
  const reach = Math.max(0, ...rank);
  rank.forEach((value, i) => {
    if (value < 0) rank[i] = reach + 1;
  });
  const columns: number[][] = Array.from({ length: Math.max(...rank) + 1 }, () => []);
  rank.forEach((r, i) => columns[r].push(i));
  const tallest = Math.max(1, ...columns.map((column) => column.length));
  const heightOf = (count: number) => count * H + (count - 1) * GY;
  const body = heightOf(tallest);
  const nodes: Node[] = [];
  columns.forEach((column, r) =>
    column.forEach((i, k) => {
      nodes[i] = {
        key: sections[i].title,
        title: sections[i].title,
        agents: sections[i].agents ?? [],
        owner: sections[i].owner,
        x: PAD + r * (W + GX),
        y: PAD + (body - heightOf(column.length)) / 2 + k * (H + GY),
        rank: r,
      };
    }),
  );

  const forward = raw.flatMap((edges) => edges).filter((edge) => rank[edge.b!] > rank[edge.a]);
  const backward = raw.flatMap((edges) => edges).filter((edge) => rank[edge.b!] <= rank[edge.a]);
  const outs = sections.map(() => 0);
  const ins = sections.map(() => 0);
  const outCount = sections.map((_, i) => forward.filter((edge) => edge.a === i).length);
  const inCount = sections.map((_, i) => forward.filter((edge) => edge.b === i).length);

  const edges: Edge[] = [];
  let loop = 0;
  for (const edge of forward) {
    const a = nodes[edge.a];
    const b = nodes[edge.b!];
    const y1 = a.y + H / 2 + (outs[a.rank]++ - (outCount[edge.a] - 1) / 2) * 12;
    const y2 = b.y + H / 2 + (ins[b.rank]++ - (inCount[edge.b!] - 1) / 2) * 12;
    const x1 = a.x + W;
    const x2 = b.x;
    const mx = (x1 + x2) / 2;
    edges.push({
      from: a.key,
      to: b.key,
      when: edge.when ?? null,
      back: false,
      path: `M${x1} ${y1} C${mx} ${y1} ${mx} ${y2} ${x2 - 2} ${y2}`,
      label: edge.when
        ? { x: mx, y: (y1 + y2) / 2 - 6, text: cut(edge.when, 22) }
        : null,
    });
  }
  for (const edge of backward) {
    const a = nodes[edge.a];
    const b = nodes[edge.b!];
    const y = PAD + body + 10 + loop * 18;
    loop += 1;
    const x1 = a.x + W / 2;
    const x2 = b.x + W / 2;
    const same = edge.a === edge.b;
    const path = same
      ? `M${a.x + W} ${a.y + 18} C${a.x + W + 46} ${a.y + 18} ${a.x + W + 46} ${a.y + 44} ${a.x + W + 2} ${a.y + 44}`
      : `M${x1} ${a.y + H} C${x1} ${y} ${x2} ${y} ${x2} ${b.y + H + 2}`;
    edges.push({
      from: a.key,
      to: b.key,
      when: edge.when ?? null,
      back: true,
      path,
      label: edge.when
        ? { x: same ? a.x + W + 30 : (x1 + x2) / 2, y: same ? a.y + 34 : y - 5, text: cut(edge.when, 30) }
        : null,
    });
  }

  return {
    nodes,
    edges,
    width: PAD * 2 + (Math.max(...rank) + 1) * W + Math.max(...rank) * GX,
    height: PAD + body + 24 + backward.length * 18 + PAD,
  };
}

const cut = (text: string, limit: number) =>
  text.length > limit ? `${text.slice(0, limit - 1)}…` : text;

/* -------------------------------------------------------------------- screen --- */

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
  const [view, setView] = useState<View>("both");
  const [selected, setSelected] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const undo = useRef<string[]>([]);
  const area = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    void loadWorkflow(workspaceId).then((next) => {
      setDoc(next);
      setSource(next?.source ?? "");
      undo.current = [];
      setSelected(null);
    });
  }, [workspaceId]);

  const sections = doc?.sections ?? [];
  const dirty = doc !== null && source !== doc.source;

  /** Section titles and the agents the document names, for the inspector's chips. */
  const names = useMemo(() => agents.map((agent) => agent.name), [agents]);
  /** One list, so "No problems found" cannot sit beside a warning. */
  const checks = useMemo(() => {
    const list: { level: "bad" | "warn"; message: string }[] = (doc?.errors ?? []).map((message) => ({
      level: "bad" as const,
      message,
    }));
    for (const section of sections) {
      if (section.owner === "agent" && (section.agents ?? []).length === 0 && !section.anyAgent) {
        list.push({
          level: "bad",
          message: `\`${section.title}\` is worked by an agent, but names none. Write \`Agent: <agent name>\` or \`Agent: any\`.`,
        });
      }
      for (const name of section.agents ?? []) {
        if (name.toLowerCase() !== "any" && !names.some((known) => known.toLowerCase() === name.toLowerCase())) {
          list.push({
            level: "warn",
            message: `\`${section.title}\` names \`${name}\`, which is not an agent in this workspace.`,
          });
        }
      }
      const titles = new Set(sections.map((entry) => entry.title));
      for (const move of section.transitions ?? []) {
        if (!titles.has(move.title)) {
          list.push({ level: "bad", message: `\`${section.title}\` moves to \`${move.title}\`, which has no section.` });
        }
      }
    }
    return list;
  }, [doc, sections, names]);

  function change(next: string, remember = true) {
    if (remember) undo.current = [...undo.current.slice(-199), source];
    setSource(next);
    setNote("");
  }

  function edit(title: string, mutate: (state: Editable) => Editable) {
    const section = sectionOf(sections, title);
    if (!section) return;
    change(apply(source, title, mutate(editable(section))));
  }

  async function save() {
    setBusy(true);
    setNote("");
    try {
      const saved = await saveWorkflow(workspaceId, source);
      setDoc(saved);
      setSource(saved.source);
      undo.current = [];
      setNote("Saved WORKFLOWS.md");
      onSaved?.();
    } catch (failure) {
      setNote(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  }

  const shown = selected ? sectionOf(sections, selected) : undefined;
  const diagram = useMemo(() => layout(sections), [sections]);

  return (
    <section className="screen fill">
      <PageHeader
        title="Workflow"
        subtitle="defined in WORKFLOWS.md"
        actions={
          <>
            {note ? <span className="dim small">{note}</span> : null}
            <div className="seg" role="group" aria-label="Layout">
              {([
                ["both", "Both"],
                ["md", "Markdown"],
                ["diagram", "Diagram"],
              ] as [View, string][]).map(([key, label]) => (
                <button key={key} type="button" aria-pressed={view === key} onClick={() => setView(key)}>
                  {label}
                </button>
              ))}
            </div>
            <button className="primary" disabled={!dirty || busy} onClick={() => void save()}>
              Save
            </button>
          </>
        }
      />

      <div className="wf-body">
        <div className={`wf wf-${view}`}>
          <section className="wf-src" aria-label="WORKFLOWS.md">
            <div className="wf-h">
              <span className="mono od-fill">WORKFLOWS.md</span>
              {dirty ? <span className="chip warn">Unsaved</span> : null}
              {dirty ? (
                <button
                  className="linkbtn"
                  onClick={() => {
                    setSource(doc?.source ?? "");
                    setNote("");
                  }}
                >
                  Revert
                </button>
              ) : null}
              {undo.current.length > 0 ? (
                <button
                  className="linkbtn"
                  onClick={() => {
                    const previous = undo.current[undo.current.length - 1];
                    undo.current = undo.current.slice(0, -1);
                    setSource(previous);
                  }}
                >
                  Undo edit
                </button>
              ) : null}
            </div>
            <textarea
              ref={area}
              className="textarea mono"
              spellCheck={false}
              value={source}
              onChange={(event) => change(event.target.value)}
              aria-label="WORKFLOWS.md contents"
            />
          </section>

          <section className="wf-out" aria-label="Diagram preview">
            {shown ? (
              <Inspector
                section={shown}
                names={names}
                onToggle={(label) =>
                  edit(shown.title, (state) => {
                    const any = label === "Any agent" || label === "Any idle agent";
                    if (state.owner === "human") {
                      return { ...state, owner: "agent", agents: [], any, moves: state.moves };
                    }
                    const named = state.agents.filter((name) => name.toLowerCase() !== label.toLowerCase());
                    const next = { ...state, agents: named, any: any ? !state.any : state.any };
                    const empty = next.agents.length === 0 && !next.any;
                    return { ...next, owner: empty ? "human" : "agent" };
                  })
                }
                onMove={(index, patch) =>
                  edit(shown.title, (state) => ({
                    ...state,
                    moves: state.moves.map((move, i) => (i === index ? { ...move, ...patch } : move)),
                  }))
                }
                onAdd={() =>
                  edit(shown.title, (state) => ({
                    ...state,
                    moves: [...state.moves, { to: sections[0]?.title ?? "Backlog", when: "" }],
                  }))
                }
                onRemove={(index) =>
                  edit(shown.title, (state) => ({
                    ...state,
                    moves: state.moves.filter((_, i) => i !== index),
                  }))
                }
                onClose={() => setSelected(null)}
                sections={sections}
              />
            ) : null}

            <div className="od-stack tight">
              <div className="od-row">
                <span className="field-label od-fill">State diagram</span>
                <span className="dim small">
                  {sections.length} {sections.length === 1 ? "status" : "statuses"} ·{" "}
                  {diagram.edges.length} {diagram.edges.length === 1 ? "transition" : "transitions"}
                </span>
              </div>
              {sections.length ? (
                <>
                  <div className="wf-svgwrap">
                    <svg
                      className="wf-svg"
                      width={diagram.width}
                      height={diagram.height}
                      viewBox={`0 0 ${diagram.width} ${diagram.height}`}
                      role="img"
                      aria-label={`Workflow diagram with ${sections.length} statuses and ${diagram.edges.length} transitions`}
                    >
                      <defs>
                        <marker
                          id="wf-arrow"
                          viewBox="0 0 10 10"
                          refX="9"
                          refY="5"
                          markerWidth="7"
                          markerHeight="7"
                          orient="auto-start-reverse"
                        >
                          <path d="M1 1.5L9 5L1 8.5z" fill="currentColor" />
                        </marker>
                      </defs>
                      {diagram.edges.map((edge, i) => (
                        <g key={`${edge.from}-${edge.to}-${i}`}>
                          <path className="wf-edge" d={edge.path} markerEnd="url(#wf-arrow)" />
                          <path
                            className="wf-hit"
                            d={edge.path}
                            onClick={() => setSelected(edge.from)}
                            role="button"
                            tabIndex={0}
                            aria-label={`Edit the move from ${edge.from} to ${edge.to}`}
                          />
                          {edge.label ? (
                            <text className="wf-lab" x={edge.label.x} y={edge.label.y} textAnchor="middle">
                              <title>{edge.when ?? ""}</title>
                              {edge.label.text}
                            </text>
                          ) : null}
                        </g>
                      ))}
                      {diagram.nodes.map((node) => {
                        const agent = node.owner === "agent";
                        const final = !(sectionOf(sections, node.key)?.transitions ?? []).length;
                        const sub = agent
                          ? (node.agents ?? []).join(", ").replace(/@/g, "") || "any agent"
                          : "You";
                        return (
                          <g
                            key={node.key}
                            className={`wf-node${agent ? " ag" : ""}${final ? " end" : ""}${selected === node.key ? " sel" : ""}`}
                            transform={`translate(${node.x} ${node.y})`}
                            onClick={() => setSelected(node.key)}
                            role="button"
                            tabIndex={0}
                            aria-label={`Edit ${node.title}`}
                          >
                            <title>{`${node.title} · ${sub}`}</title>
                            <rect width={W} height={H} rx="10" />
                            <text className="t" x={W / 2} y={sub ? 24 : 34} textAnchor="middle">
                              {cut(node.title, 19)}
                            </text>
                            {sub ? (
                              <text className="s" x={W / 2} y={42} textAnchor="middle">
                                {cut(sub, 26)}
                              </text>
                            ) : null}
                          </g>
                        );
                      })}
                    </svg>
                  </div>
                  <p className="dim small">
                    Not what you meant? Click a status or an arrow to correct it, or edit the file on the left.
                  </p>
                  <div className="wf-key">
                    <span>
                      <i className="ag" />
                      An agent handles it
                    </span>
                    <span>
                      <i />
                      You handle it
                    </span>
                    <span>
                      <i className="end" />
                      Final status
                    </span>
                  </div>
                </>
              ) : (
                <div className="empty">
                  <strong>Nothing to draw yet</strong>
                  Add a “## Status” heading in WORKFLOWS.md.
                </div>
              )}
            </div>

            <div className="od-stack tight">
              <span className="field-label">Checks</span>
              {checks.length === 0 ? (
                <span className="chip ok">
                  <Icon name="check" size={12} />
                  No problems found
                </span>
              ) : (
                <ul className="wf-iss">
                  {checks.map((check, i) => (
                    <li key={i}>
                      <span className={check.level === "bad" ? "chip bad" : "chip warn"}>
                        {check.level === "bad" ? "Error" : "Check"}
                      </span>
                      <span dangerouslySetInnerHTML={{ __html: check.message }} />
                    </li>
                  ))}
                </ul>
              )}
            </div>

            {diagram.edges.length > 0 ? (
              <div className="od-stack tight">
                <span className="field-label">Transitions</span>
                <div className="md-tablewrap">
                  <table className="wf-table">
                    <thead>
                      <tr>
                        <th>From</th>
                        <th>To</th>
                        <th>Handled by</th>
                        <th>When</th>
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      {sections.flatMap((section) =>
                        (section.transitions ?? []).map((move, i) => (
                          <tr key={`${section.title}-${move.title}-${i}`}>
                            <td>{section.title}</td>
                            <td>{move.title}</td>
                            <td>{who(section)}</td>
                            <td>{move.when ?? "—"}</td>
                            <td>
                              <button className="linkbtn" onClick={() => setSelected(section.title)}>
                                Edit
                              </button>
                            </td>
                          </tr>
                        )),
                      )}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}
          </section>
        </div>
      </div>
    </section>
  );
}

/** Who works a status, as the table prints it. */
function who(section: WorkflowSection): string {
  const agents = (section.agents ?? []).map((name) => name.replace(/@/g, ""));
  if (section.owner !== "agent") return "You";
  if (agents.length === 0) return section.anyAgent ? "Any agent" : "—";
  return agents.join(", ");
}

/* ----------------------------------------------------------------- inspector --- */

function Inspector({
  section,
  sections,
  names,
  onToggle,
  onMove,
  onAdd,
  onRemove,
  onClose,
}: {
  section: WorkflowSection;
  sections: WorkflowSection[];
  names: string[];
  onToggle: (label: string) => void;
  onMove: (index: number, patch: { to?: string; when?: string }) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  onClose: () => void;
}) {
  const state = editable(section);
  const on = (label: string) =>
    label === "Any agent" || label === "Any idle agent"
      ? state.any
      : state.agents.some((name) => name.toLowerCase() === label.toLowerCase());
  const chips = ["You", "Any agent", ...names];

  return (
    <div className="wf-insp">
      <div className="od-row">
        <strong className="od-fill">{section.title}</strong>
        <button className="icon-btn" onClick={onClose} aria-label="Close">
          <Icon name="close" size={14} />
        </button>
      </div>

      <div className="od-stack tight">
        <span className="field-label">How this was read</span>
        <p className="small">{say(section)}</p>
      </div>

      <div className="od-stack tight">
        <span className="field-label">Handled by</span>
        <div className="tgl-group" role="group" aria-label="Handled by">
          {chips.map((label) => (
            <button
              key={label}
              type="button"
              className={on(label) ? "tgl on" : "tgl"}
              aria-pressed={on(label)}
              onClick={() => onToggle(label)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="od-stack tight">
        <span className="field-label">Moves to</span>
        {state.moves.length === 0 ? (
          <span className="dim small">Nowhere yet. This is a final status.</span>
        ) : (
          state.moves.map((move, i) => (
            <div className="wf-row" key={i}>
              <select
                className="select"
                value={move.to}
                onChange={(event) => onMove(i, { to: event.target.value })}
                aria-label="Moves to"
              >
                {sections.map((entry) => (
                  <option key={entry.title} value={entry.title}>
                    {entry.title}
                  </option>
                ))}
                {sections.some((entry) => entry.title === move.to) ? null : (
                  <option value={move.to}>{move.to} (not defined)</option>
                )}
              </select>
              <input
                className="input"
                value={move.when}
                placeholder="When it moves"
                aria-label={`When it moves to ${move.to}`}
                onChange={(event) => onMove(i, { when: event.target.value })}
              />
              <button className="icon-btn" onClick={() => onRemove(i)} aria-label="Remove transition">
                <Icon name="close" size={14} />
              </button>
            </div>
          ))
        )}
        <button className="btn" onClick={onAdd} style={{ justifySelf: "start" }}>
          <Icon name="plus" size={14} />
          Add transition
        </button>
      </div>
    </div>
  );
}

/** The prototype's plain-language reading of a section, which catches misreadings. */
function say(section: WorkflowSection): string {
  const agents = (section.agents ?? []).map((name) => name.replace(/@/g, ""));
  const by = section.owner === "agent" ? (agents.length ? agents.join(", ") : "an agent") : "you";
  const moves = (section.transitions ?? []).map(
    (move) => `“${move.title}”${move.when ? ` when ${move.when}` : ""}`,
  );
  return `Work in “${section.title}” is handled by ${by}. ${
    moves.length ? `It can move to ${moves.join(", ")}.` : "It is a final status, nothing follows it."
  }`;
}
