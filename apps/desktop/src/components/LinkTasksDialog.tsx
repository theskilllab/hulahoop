import { useEffect, useRef, useState } from "react";
import type { IssueSummary } from "../api";
import { plural } from "./TaskBits";

/** Pick any number of tasks to file under a milestone; a task already filed elsewhere is moved. */
export default function LinkTasksDialog({
  candidates,
  milestoneTitles,
  busy,
  onLink,
  onClose,
}: {
  candidates: IssueSummary[];
  milestoneTitles: Map<string, string>;
  busy: boolean;
  onLink: (issueIds: string[]) => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());

  useEffect(() => {
    dialog.current?.showModal();
    return () => dialog.current?.close();
  }, []);

  const needle = query.trim().toLowerCase();
  const shown = candidates.filter((issue) => `${issue.key} ${issue.title}`.toLowerCase().includes(needle));
  const toggle = (id: string) =>
    setPicked((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });

  return (
    <dialog
      ref={dialog}
      aria-labelledby="link-tasks-title"
      className="settings-dialog link-tasks-dialog"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <h2 id="link-tasks-title">Link tasks</h2>
      <input
        type="search"
        autoFocus
        aria-label="Filter tasks"
        placeholder="Filter by key or title…"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <ul className="lk-list">
        {shown.map((issue) => (
          <li key={issue.id}>
            <label>
              <input type="checkbox" checked={picked.has(issue.id)} onChange={() => toggle(issue.id)} />
              <span className="mono">{issue.key}</span>
              <span className="lk-title">{issue.title}</span>
              {issue.milestoneId && (
                <span className="chip">in {milestoneTitles.get(issue.milestoneId) ?? "another milestone"}</span>
              )}
            </label>
          </li>
        ))}
        {shown.length === 0 && <li className="empty">No tasks to link.</li>}
      </ul>
      <div className="ms-actions">
        <button type="button" className="secondary" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="primary"
          disabled={busy || picked.size === 0}
          onClick={() => onLink([...picked])}
        >
          {picked.size === 0 ? "Link tasks" : `Link ${plural(picked.size, "task")}`}
        </button>
      </div>
    </dialog>
  );
}
