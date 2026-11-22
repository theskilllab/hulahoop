import { useEffect, useMemo, useRef, useState } from "react";

import { loadWorkflow, saveWorkflow, type Agent, type WorkflowDoc, type WorkflowSection } from "../api";
import Icon from "../Icon";
import RichText from "../RichText";
import { MetaLine } from "../metaLine";
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
  /** A person is named on the `Users:` line, alongside any agents. */
  people: boolean;
  agents: string[];
  any: boolean;
  moves: { to: string; when: string }[];
}

const KEYWORDS = /^(users|user|next|when)\s*:/i;

/** Where the caret sits inside a textarea, in the textarea's own coordinates. */
/**
 * Where the caret is, in the textarea's own coordinates.
 *
 * A mirror of the text is laid over the textarea itself rather than appended to
 * the page, so padding and border cancel out, and the textarea's own scroll is
 * subtracted. Without that subtraction the caret of a long file reports its
 * height in the document and the `@` list walks off the bottom of the pane — which
 * is exactly what happened once a document grew past one screen.
 */
function caretPoint(
  area: HTMLTextAreaElement,
  index: number,
): { x: number; y: number; height: number; line: number } {
  const style = window.getComputedStyle(area);
  const rect = area.getBoundingClientRect();
  const mirror = document.createElement("div");
  mirror.style.position = "absolute";
  mirror.style.visibility = "hidden";
  mirror.style.pointerEvents = "none";
  mirror.style.left = `${rect.left + window.scrollX}px`;
  mirror.style.top = `${rect.top + window.scrollY}px`;
  mirror.style.whiteSpace = "pre-wrap";
  mirror.style.overflowWrap = "break-word";
  mirror.style.width = `${area.clientWidth}px`;
  mirror.style.fontFamily = style.fontFamily;
  mirror.style.fontSize = style.fontSize;
  mirror.style.fontWeight = style.fontWeight;
  mirror.style.lineHeight = style.lineHeight;
  mirror.style.letterSpacing = style.letterSpacing;
  mirror.style.padding = style.padding;
  mirror.style.border = style.border;
  mirror.style.boxSizing = style.boxSizing;
  mirror.textContent = area.value.slice(0, index);
  const marker = document.createElement("span");
  marker.textContent = "\u200b";
  mirror.appendChild(marker);
  document.body.appendChild(mirror);
  const line = Number.parseFloat(style.lineHeight || "18") || 18;
  const point = {
    x: marker.offsetLeft - area.scrollLeft,
    y: marker.offsetTop - area.scrollTop,
    height: area.clientHeight,
    line,
  };
  mirror.remove();
  return point;
}

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

/**
 * The `Users:` line: who handles the status, in one sentence. `human` means a
 * person, a name means that agent, `any` means whichever agent the project binds
 * to the column. A status with no agents is a person's column.
 */
function usersLine(state: Editable): string[] {
  if (state.any) return ["Users: any"];
  const names = [
    ...(state.people ? ["human"] : []),
    ...state.agents.map((name) => `@${name}`),
  ];
  return [`Users: ${names.length ? names.join(", ") : "human"}`];
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
  const written = [...usersLine(state), ...nextLines(state)];
  return [...lines.slice(0, spot.start), ...written, ...lines.slice(spot.end)].join("\n");
}

/** The prose under a section's metadata: everything from the first blank line after it. */
function bodyRange(source: string, title: string): { from: number; to: number } | null {
  const spot = block(source, title);
  if (!spot) return null;
  const lines = source.split("\n");
  let from = spot.end;
  while (from < lines.length && lines[from].trim() === "") from += 1;
  let to = from;
  while (to < lines.length && !/^##\s/.test(lines[to])) to += 1;
  while (to > from && lines[to - 1].trim() === "") to -= 1;
  return { from, to };
}

/**
 * Replace one section's prose, leaving its metadata and every other section alone.
 *
 * This is what lets a rich editor sit above a file whose first lines are machine
 * metadata: markdown goes back exactly where the paragraph was, and `Users:` and
 * `Next:` never travel through a rich document.
 */
function applyBody(source: string, title: string, markdown: string): string {
  const spot = block(source, title);
  const range = bodyRange(source, title);
  if (!spot || !range) return source;
  const lines = source.split("\n");
  const written = markdown.replace(/\s+$/, "").split("\n").filter((line, i, all) =>
    !(i === all.length - 1 && line === ""),
  );
  const head = lines.slice(0, range.from);
  const tail = lines.slice(range.to);
  const gap = written.length > 0 ? [""] : [];
  const after = tail.length > 0 ? [""] : [];
  return [...head, ...gap, ...written, ...after, ...tail].join("\n");
}

const PEOPLE = new Set(["human", "you", "me", "i", "person", "people", "user", "us"]);

function editable(section: WorkflowSection): Editable {
  const named = section.agents ?? [];
  const line = section.users ?? [];
  return {
    owner: section.owner === "agent" ? "agent" : "human",
    people: line.some((entry) => PEOPLE.has(entry.toLowerCase())),
    agents: named.filter((name) => name.toLowerCase() !== "any"),
    any: section.anyAgent ?? named.some((name) => name.toLowerCase() === "any"),
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
/** Space between wrapped rows of ranks. */
const ROW_GAP = 56;

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

/**
 * Place the statuses in a layered grid that fits the pane it is drawn in.
 *
 * The prototype's canvas is one long row per rank, which is fine on a page of its
 * own and cut off inside a split view. Ranks are wrapped into rows instead: as
 * many columns as the pane fits, then the next row, so the whole workflow is
 * visible at its natural size rather than scaled down to unreadable type.
 */
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
  // How many rank columns fit side by side; at least two, and one per rank when
  // the pane is wide enough.
  // One row: the flow reads left to right, and the zoom fits it to the pane.
  const perRow = Math.max(1, columns.length);
  const rows = 1;
  const nodes: Node[] = [];
  columns.forEach((column, r) => {
    const row = Math.floor(r / perRow);
    const slot = r % perRow;
    column.forEach((i, k) => {
      nodes[i] = {
        key: sections[i].title,
        title: sections[i].title,
        agents: sections[i].agents ?? [],
        owner: sections[i].owner,
        x: PAD + slot * (W + GX),
        y: PAD + row * (body + ROW_GAP) + (body - heightOf(column.length)) / 2 + k * (H + GY),
        rank: r,
      };
    });
  });

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
    const y = a.y + H + 16 + loop * 16;
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
    width: PAD * 2 + Math.min(perRow, columns.length) * W + (Math.min(perRow, columns.length) - 1) * GX,
    height: PAD * 2 + rows * body + (rows - 1) * ROW_GAP + backward.length * 18,
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
  // The file opens as formatted text and keeps a source view, because `Users:`
  // and `Next:` are lines the parser reads and sometimes want typing by hand.
  const [rich, setRich] = useState<boolean>(() => localStorage.getItem("hulahoop.wf-editor") !== "source");
  const [ratio, setRatio] = useState<number>(() => Number(localStorage.getItem("hulahoop.wf-ratio") ?? 0.42));
  // null means "fit the pane", which is what a fresh screen shows.
  const [zoom, setZoom] = useState<number | null>(null);
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

  /** Drag the divider between the file and the diagram. */
  function startResize(event: React.PointerEvent<HTMLDivElement>) {
    const pane = event.currentTarget.parentElement;
    if (!pane) return;
    const box = pane.getBoundingClientRect();
    const move = (pointer: PointerEvent) => {
      const share = Math.min(0.75, Math.max(0.2, (pointer.clientX - box.left) / box.width));
      setRatio(share);
      localStorage.setItem("hulahoop.wf-ratio", String(share));
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
  }

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

  const [mention, setMention] = useState<
    { start: number; query: string; x: number; y: number; above: boolean; index: number } | null
  >(null);

  /** `@` at the caret opens the team up: type to filter, arrows and Enter to pick. */
  function trackMention(value: string, caret: number) {
    const before = value.slice(0, caret);
    const match = /(?:^|\s)@([\w -]*)$/.exec(before);
    if (!match || !area.current) return setMention(null);
    const point = caretPoint(area.current, caret);
    const below = point.y + point.line + 232;
    setMention({
      start: caret - match[1].length - 1,
      query: match[1],
      x: point.x,
      y: point.y,
      // More room above than below, and the caret is past the halfway mark: open
      // upwards so the list is never clipped by the pane.
      above: below > point.height && point.y > point.height - point.y,
      index: 0,
    });
  }

  function pick(name: string) {
    if (!mention || !area.current) return;
    const caret = area.current.selectionStart;
    const next = `${source.slice(0, mention.start)}@${name} ${source.slice(caret)}`;
    setMention(null);
    change(next);
    requestAnimationFrame(() => {
      const at = mention.start + name.length + 2;
      area.current?.focus();
      area.current?.setSelectionRange(at, at);
    });
  }

  const candidates = mention
    ? names.filter((name) => name.toLowerCase().includes(mention.query.trim().toLowerCase())).slice(0, 8)
    : [];

  const shown = selected ? sectionOf(sections, selected) : undefined;
  // The diagram is laid out to the width it actually gets, so a split pane wraps
  // the ranks into a second row instead of cutting them off.
  const [wrapEl, setWrapEl] = useState<HTMLDivElement | null>(null);
  const [avail, setAvail] = useState(0);
  useEffect(() => {
    if (!wrapEl) return;
    setAvail(wrapEl.clientWidth);
    const watcher = new ResizeObserver(([entry]) => setAvail(entry.contentRect.width));
    watcher.observe(wrapEl);
    return () => watcher.disconnect();
  }, [wrapEl, view]);
  const diagram = useMemo(() => layout(sections), [sections]);
  const fit = avail > 0 ? Math.min(1, (avail - 26) / diagram.width) : 1;
  const scale = zoom ?? Math.max(0.55, Number(fit.toFixed(2)));

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
        <div
          className={`wf wf-${view}`}
          style={
            view === "both"
              ? { gridTemplateColumns: `minmax(240px, ${ratio}fr) 7px minmax(0, ${1 - ratio}fr)` }
              : undefined
          }
        >
          <section className="wf-src" aria-label="WORKFLOWS.md">
            <div className="wf-h">
              <span className="mono od-fill">WORKFLOWS.md</span>
              <button
                type="button"
                className={rich ? "linkbtn on" : "linkbtn"}
                aria-pressed={rich}
                title={rich ? "Show the file as text" : "Show the file formatted"}
                onClick={() => {
                  const next = !rich;
                  setRich(next);
                  localStorage.setItem("hulahoop.wf-editor", next ? "rich" : "source");
                }}
              >
                {rich ? "Formatted" : "Source"}
              </button>
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
            <div className="wf-edit">
              {rich ? (
                <RichText
                  value={source}
                  onChange={(markdown) => change(markdown)}
                  minHeight={480}
                  ariaLabel="WORKFLOWS.md"
                  extensions={[MetaLine]}
                  placeholder={"# Workflows\n\n## Backlog\nUsers: human\nNext: To do - an issue is ready"}
                />
              ) : (
              <textarea
                ref={area}
                className="textarea mono"
                spellCheck={false}
                value={source}
                onChange={(event) => {
                  change(event.target.value);
                  trackMention(event.target.value, event.target.selectionStart ?? 0);
                }}
                onKeyDown={(event) => {
                  if (!mention || candidates.length === 0) return;
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    setMention({ ...mention, index: (mention.index + 1) % candidates.length });
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    setMention({
                      ...mention,
                      index: (mention.index - 1 + candidates.length) % candidates.length,
                    });
                  } else if (event.key === "Enter" || event.key === "Tab") {
                    event.preventDefault();
                    pick(candidates[mention.index] ?? candidates[0]);
                  } else if (event.key === "Escape") {
                    setMention(null);
                  }
                }}
                onBlur={() => window.setTimeout(() => setMention(null), 120)}
                aria-label="WORKFLOWS.md contents"
              />
              )}
              {!rich && mention && candidates.length > 0 ? (
                <ul
                  className="wf-at"
                  style={
                    mention.above
                      ? { left: mention.x, top: mention.y, transform: "translateY(-100%) translateY(-4px)" }
                      : { left: mention.x, top: mention.y + 20 }
                  }
                  role="listbox"
                >
                  {candidates.map((name, index) => (
                    <li key={name}>
                      <button
                        type="button"
                        role="option"
                        aria-selected={index === mention.index}
                        className={index === mention.index ? "on" : undefined}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => pick(name)}
                      >
                        <Icon name="bot" /> {name}
                      </button>
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          </section>

          {view === "both" ? (
            <div
              className="wf-grip"
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize the file and the diagram"
              onPointerDown={startResize}
            />
          ) : null}

          <section className="wf-out" aria-label="Diagram preview">
            {shown ? (
              <Inspector
                section={shown}
                names={names}
                onToggle={(label) =>
                  edit(shown.title, (state) => {
                    const any = label === "Any agent" || label === "Any idle agent";
                    if (any) return { ...state, any: !state.any, people: false };
                    if (label === "You" || label === "human") {
                      return { ...state, people: !state.people };
                    }
                    const named = state.agents.filter((name) => name.toLowerCase() !== label.toLowerCase());
                    const has = named.length !== state.agents.length;
                    return {
                      ...state,
                      agents: has ? named : [...state.agents, label],
                      any: has ? state.any : false,
                    };
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
                onBody={(markdown) => {
                  const next = applyBody(source, shown.title, markdown);
                  if (next !== source) change(next);
                }}
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
                <span className="wf-zoom">
                  <button
                    type="button"
                    onClick={() => setZoom(null)}
                    aria-label="Fit to the pane"
                    title="Fit to the pane"
                  >
                    ⤢
                  </button>
                  <button
                    type="button"
                    onClick={() => setZoom(Math.max(0.5, Math.round((scale - 0.1) * 10) / 10))}
                    aria-label="Zoom out"
                    title="Zoom out"
                  >
                    −
                  </button>
                  <button
                    type="button"
                    onClick={() => setZoom(Math.min(1.8, Math.round((scale + 0.1) * 10) / 10))}
                    aria-label="Zoom in"
                    title="Zoom in"
                  >
                    +
                  </button>
                </span>
              </div>
              {sections.length ? (
                <>
                  <div className="wf-svgwrap" ref={setWrapEl}>
                    <div
                      style={{
                        width: diagram.width * scale,
                        height: diagram.height * scale,
                      }}
                    >
                    <svg
                      className="wf-svg"
                      style={{
                        transform: `scale(${scale})`,
                        transformOrigin: "top left",
                      }}
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
                          <path
                            className={edge.back ? "wf-edge back" : "wf-edge"}
                            d={edge.path}
                            markerEnd="url(#wf-arrow)"
                          />
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
                        const held = sectionOf(sections, node.key);
                        const agent =
                          (held?.agents ?? []).length > 0 || Boolean(held?.anyAgent);
                        const final = !(sectionOf(sections, node.key)?.transitions ?? []).length;
                        const sub = agent
                          ? (held?.agents ?? []).join(", ").replace(/@/g, "") || "any agent"
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
  if (agents.length > 0) return agents.join(", ");
  return section.anyAgent ? "Any agent" : "You";
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
  onBody,
  onClose,
}: {
  section: WorkflowSection;
  sections: WorkflowSection[];
  names: string[];
  onToggle: (label: string) => void;
  onMove: (index: number, patch: { to?: string; when?: string }) => void;
  onAdd: () => void;
  onRemove: (index: number) => void;
  onBody: (markdown: string) => void;
  onClose: () => void;
}) {
  const state = editable(section);
  const on = (label: string) => {
    if (label === "Any agent" || label === "Any idle agent") return state.any;
    if (label === "You" || label === "human") return state.people;
    return state.agents.some((name) => name.toLowerCase() === label.toLowerCase());
  };
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
        <span className="field-label">What it says</span>
        <RichText
          value={section.body ?? ""}
          onChange={onBody}
          minHeight={90}
          placeholder="Describe what this status means. Markdown is fine."
          ariaLabel={`Prose under ${section.title}`}
        />
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
