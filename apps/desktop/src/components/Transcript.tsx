import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { loadTranscript, type TranscriptEntry } from "../api";
import Icon from "../Icon";
import MarkdownView from "./MarkdownView";
import { Avatar, RunStatusChip, plural } from "./TaskBits";

const POLL_MS = 1500;

/** One line of a tool's input, enough to tell calls apart. */
function inputSummary(input: string | null): string {
  if (!input) return "";
  try {
    const parsed = JSON.parse(input) as unknown;
    if (parsed && typeof parsed === "object") {
      const record = parsed as Record<string, unknown>;
      const first =
        record.command ?? record.file_path ?? record.path ?? record.pattern ?? record.url ?? record.query ?? Object.values(record)[0];
      if (typeof first === "string") return first.split("\n")[0].slice(0, 160);
    }
  } catch {
    // Not JSON: show it as written.
  }
  return input.split("\n")[0].slice(0, 160);
}

/** One stdout line can yield several entries with the same `seq`, so rows key on arrival order. */
type Keyed = TranscriptEntry & { key: number };

type Row =
  | { kind: "entry"; entry: Keyed }
  | { kind: "tool"; call: Keyed; result: Keyed | null };

/** Pair each tool call with its result, so a tool reads as one row. */
function toRows(entries: Keyed[]): Row[] {
  const rows: Row[] = [];
  const open = new Map<string, Extract<Row, { kind: "tool" }>>();
  for (const entry of entries) {
    if (entry.kind === "tool_call") {
      const row = { kind: "tool" as const, call: entry, result: null };
      rows.push(row);
      if (entry.toolId) open.set(entry.toolId, row);
    } else if (entry.kind === "tool_result" && entry.toolId && open.has(entry.toolId)) {
      const row = open.get(entry.toolId)!;
      row.result = entry;
      open.delete(entry.toolId);
    } else {
      rows.push({ kind: "entry", entry });
    }
  }
  return rows;
}

function ToolRow({ call, result, live, expandAll }: { call: TranscriptEntry; result: TranscriptEntry | null; live: boolean; expandAll: boolean }) {
  const [open, setOpen] = useState<boolean | null>(null);
  const expanded = open ?? expandAll;
  const failed = result?.isError || call.isError;
  const state = result ? (failed ? "Error" : "Done") : live ? "Running…" : "No result";
  const output = result?.output ?? result?.text ?? "";
  return (
    <li className={`tx-tool${failed ? " bad" : ""}`}>
      <button type="button" className="tx-tool-head" aria-expanded={expanded} onClick={() => setOpen(!expanded)}>
        <Icon name={expanded ? "chevdown" : "chevron"} size={12} />
        <strong>{call.tool ?? "tool"}</strong>
        <code className="truncate">{inputSummary(call.input) || call.text}</code>
        <span className={`tx-state${failed ? " bad" : result ? " ok" : ""}`}>{state}</span>
      </button>
      {expanded ? (
        <div className="tx-tool-body">
          {call.input ? <pre>{call.input}</pre> : null}
          {output ? <pre className="tx-out">{output}</pre> : null}
        </div>
      ) : null}
    </li>
  );
}

/**
 * A run's transcript, normalized across harnesses: the prompt, thinking,
 * the agent's words and every tool call. While the run is live it polls for
 * new lines and keeps the view pinned to the bottom unless you scrolled up.
 */
export default function Transcript({
  runId,
  runStatus,
  agentName,
  issueKey,
  credentialSource = null,
  onBack,
}: {
  runId: string;
  runStatus: string;
  agentName: string;
  issueKey: string;
  /** What the run signed in with, such as `Claude subscription` or `host login`. */
  credentialSource?: string | null;
  onBack: () => void;
}) {
  const [entries, setEntries] = useState<Keyed[]>([]);
  const arrived = useRef(0);
  const [live, setLive] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [expandAll, setExpandAll] = useState(false);
  const cursor = useRef(0);
  const root = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const back = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    back.current?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;
    let failures = 0;
    cursor.current = 0;
    arrived.current = 0;
    setEntries([]);
    setLoaded(false);
    async function tick() {
      try {
        // Catch up in pages: the server sends at most a few hundred lines a call.
        for (let page = 0; page < 20; page++) {
          const next = await loadTranscript(runId, cursor.current);
          if (!alive) return;
          const scroller = root.current?.closest(".side-panel-body");
          if (scroller) stick.current = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 48;
          if (next.entries.length) {
            const keyed = next.entries.map((entry) => ({ ...entry, key: arrived.current++ }));
            setEntries((list) => [...list, ...keyed]);
          }
          failures = 0;
          cursor.current = next.lastSeq;
          setLive(next.live);
          setError(null);
          if (next.entries.length < 500) {
            setLoaded(true);
            if (next.live) timer = window.setTimeout(tick, POLL_MS);
            return;
          }
        }
        timer = window.setTimeout(tick, POLL_MS);
      } catch (cause) {
        if (!alive) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setLoaded(true);
        // Give up after a few misses in a row; reopening the transcript retries.
        if (++failures < 5) timer = window.setTimeout(tick, POLL_MS * 2);
      }
    }
    void tick();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [runId]);

  useLayoutEffect(() => {
    if (!stick.current) return;
    const scroller = root.current?.closest(".side-panel-body");
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [entries]);

  const rows = useMemo(() => toRows(entries), [entries]);
  const toolCalls = entries.filter((entry) => entry.kind === "tool_call").length;
  const turns = entries.filter((entry) => entry.kind === "text").length;

  return (
    <div className="transcript" ref={root}>
      <div className="tx-head">
        <button ref={back} type="button" className="linkbtn tx-back" onClick={onBack}>
          ← Back to {issueKey}
        </button>
        <h2>
          Transcript <span className="dim">· {agentName}</span>
        </h2>
      </div>
      <div className="tx-bar">
        <RunStatusChip status={live ? (runStatus === "queued" ? "queued" : "running") : runStatus === "running" ? "succeeded" : runStatus} />
        <span className="dim small">
          {plural(turns, "turn")} · {plural(toolCalls, "tool call")}
          {credentialSource ? ` · Credential: ${credentialSource}` : null}
        </span>
        <span className="spacer" />
        <label className="check small">
          <input type="checkbox" checked={expandAll} onChange={(event) => setExpandAll(event.target.checked)} />
          Expand tool output
        </label>
      </div>
      {error ? <p className="error">{error}</p> : null}
      {loaded && entries.length === 0 && !error ? (
        <p className="tx-note dim">{live ? "Waiting for the agent's first words…" : "This run left no transcript."}</p>
      ) : null}
      <ol className="tx-list">
        {rows.map((row) =>
          row.kind === "tool" ? (
            <ToolRow key={row.call.key} call={row.call} result={row.result} live={live} expandAll={expandAll} />
          ) : (
            <TranscriptLine key={row.entry.key} entry={row.entry} issueKey={issueKey} agentName={agentName} />
          ),
        )}
      </ol>
      {live && loaded ? (
        <p className="tx-live tx-note">
          <span className="pulse" aria-hidden="true" /> Live — new lines appear as the agent works
        </p>
      ) : null}
    </div>
  );
}

function TranscriptLine({ entry, issueKey, agentName }: { entry: TranscriptEntry; issueKey: string; agentName: string }) {
  switch (entry.kind) {
    case "prompt":
      return (
        <li className="tx-prompt">
          <details>
            <summary>Task prompt · {issueKey}</summary>
            <pre>{entry.text}</pre>
          </details>
        </li>
      );
    case "thinking":
      return (
        <li className="tx-think">
          <details>
            <summary>Thinking</summary>
            <p>{entry.text}</p>
          </details>
        </li>
      );
    case "text":
      return (
        <li className="tx-text tx-agent">
          <Avatar name={agentName} kind="agent" />
          <MarkdownView value={entry.text} />
        </li>
      );
    case "error":
      return <li className="tx-error">{entry.text}</li>;
    case "done":
      return <li className="tx-done tx-note dim">{entry.text || "Run finished"}</li>;
    default:
      return (
        <li className="tx-text">
          <pre className="tx-out">{entry.output ?? entry.text}</pre>
        </li>
      );
  }
}
