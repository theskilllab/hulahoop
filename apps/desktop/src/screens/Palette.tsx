import { useEffect, useMemo, useRef, useState } from "react";

import type { Agent, IssueSummary as Issue } from "../api";
import Icon from "../Icon";

export interface PaletteTarget {
  kind: "view" | "issue" | "agent";
  id: string;
  label: string;
  hint: string;
}

const VIEWS: { id: string; label: string; hint: string }[] = [
  { id: "inbox", label: "Inbox", hint: "tasks waiting on you" },
  { id: "tasks", label: "Tasks", hint: "the board" },
  { id: "projects", label: "Projects", hint: "prefixes and repositories" },
  { id: "milestones", label: "Milestones", hint: "goals and their artifacts" },
  { id: "artifacts", label: "Artifacts", hint: "documents and files" },
  { id: "agents", label: "Agents", hint: "who does the work" },
  { id: "skills", label: "Skills", hint: "the shared library" },
  { id: "workflow", label: "Workflow", hint: "statuses, owners, transitions" },
  { id: "connectors", label: "Connectors", hint: "logins and keys" },
  { id: "cost", label: "Cost control", hint: "what the agents spend" },
  { id: "prs", label: "Pull requests", hint: "what is open for review" },
  { id: "activity", label: "Activity", hint: "recent runs" },
  { id: "new-artifact", label: "Create new artifact", hint: "a plan, spec, report, notes or file" },
  { id: "settings", label: "Settings", hint: "theme, workspace, Lumi" },
];

/**
 * ⌘K. One field over the things a person navigates to: the views, the tasks by key
 * or title, the agents by name. It is a jump list, not a command runner — every
 * entry takes you somewhere that already exists.
 */
export default function Palette({
  issues,
  agents,
  onPick,
  onClose,
}: {
  issues: Issue[];
  agents: Agent[];
  onPick: (target: PaletteTarget) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
  }, []);

  const results = useMemo<PaletteTarget[]>(() => {
    const needle = query.trim().toLowerCase();
    const views = VIEWS.map((entry) => ({
      kind: "view" as const,
      id: entry.id,
      label: entry.label,
      hint: entry.hint,
    }));
    // Done and cancelled tasks are findable too, after the open ones.
    const finished = (issue: Issue) => issue.status === "done" || issue.status === "cancelled";
    const tasks = [...issues.filter((issue) => !finished(issue)), ...issues.filter(finished)]
      .map((issue) => ({
        kind: "issue" as const,
        id: issue.id,
        label: `${issue.key} · ${issue.title}`,
        hint: issue.status.replace(/_/g, " "),
      }));
    const finishedIds = new Set(issues.filter(finished).map((issue) => issue.id));
    const people = agents.map((agent) => ({
      kind: "agent" as const,
      id: agent.id,
      label: agent.name,
      hint: `${agent.adapter} · ${agent.model || "default model"}`,
    }));
    const all = [...views, ...tasks, ...people];
    if (!needle) return [...views, ...tasks.filter((entry) => !finishedIds.has(entry.id)), ...people].slice(0, 24);
    return all
      .filter((entry) => `${entry.label} ${entry.hint}`.toLowerCase().includes(needle))
      .slice(0, 24);
  }, [query, issues, agents]);

  useEffect(() => {
    setIndex(0);
  }, [query]);

  function key(event: React.KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setIndex((was) => Math.min(was + 1, results.length - 1));
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setIndex((was) => Math.max(was - 1, 0));
    }
    if (event.key === "Enter" && results[index]) {
      event.preventDefault();
      onPick(results[index]);
    }
  }

  return (
    <div className="dialog" role="dialog" aria-modal="true" onClick={onClose}>
      <div
        className="palette"
        onClick={(event) => {
          event.stopPropagation();
        }}
      >
        <div className="palette-field">
          <Icon name="search" size={15} />
          <input
            ref={input}
            value={query}
            placeholder="Jump to a task, an agent, a view…"
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={key}
          />
          <kbd>esc</kbd>
        </div>
        <div className="palette-list" role="listbox">
          {results.length === 0 ? <p className="dim small">Nothing matches that.</p> : null}
          {results.map((entry, position) => (
            <button
              key={`${entry.kind}-${entry.id}`}
              role="option"
              aria-selected={position === index}
              className="pitem"
              onMouseEnter={() => setIndex(position)}
              onClick={() => onPick(entry)}
            >
              <Icon
                name={entry.kind === "issue" ? "tasks" : entry.kind === "agent" ? "bot" : "folder"}
                size={14}
              />
              <span className="fill truncate">{entry.label}</span>
              <span className="dim tiny truncate">{entry.hint}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
