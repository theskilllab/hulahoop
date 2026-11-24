import { useEffect, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { createIssue, uploadArtifact, type Agent, type IssueSummary, type Milestone, type Project } from "../api";
import Icon from "../Icon";
import RichText from "../RichText";
import { AssigneeOptions, Avatar, PriorityIcon, PriorityOptions, parseAssigneeKey, priorityLabel } from "./TaskBits";

const MAX_TITLE = 80;

/** Characters markdown can escape; held aside while marks are stripped. */
const MARK_CHARS = /[\\`*_{}[\]()#+\-.!|<>~]/g;
const HIDE_BASE = 0xe000;
const hide = (text: string) => text.replace(MARK_CHARS, (char) => String.fromCharCode(HIDE_BASE + char.charCodeAt(0)));
const unhide = (text: string) => text.replace(/[\ue000-\ue07f]/g, (char) => String.fromCharCode(char.charCodeAt(0) - HIDE_BASE));

/**
 * Markdown's marks on a title line: a heading or list marker, backslash
 * escapes, inline marks (`**bold**`, `*em*`, `_em_`, `` `code` ``,
 * `~~strike~~`, `[text](url)`) and a trailing hard-break backslash.
 */
function plainTitle(line: string): string {
  return unhide(
    line
      .replace(/^(#{1,6}|>|[-*+]|\d+[.)])\s+/, "")
      // Escaped characters and code spans are literal text: hide them from the mark rules below.
      .replace(/\\([\\`*_{}[\]()#+\-.!|<>~])/g, (_, char: string) => hide(char))
      .replace(/`([^`]+)`/g, (_, code: string) => hide(code))
      .replace(/!?\[([^\]]*)\]\((?:[^()]|\([^()]*\))*\)/g, "$1")
      .replace(/(\*\*|__)(\S(?:.*?\S)?)\1/g, "$2")
      .replace(/~~(\S(?:.*?\S)?)~~/g, "$1")
      .replace(/\*(\S(?:.*?\S)?)\*/g, "$1")
      .replace(/(^|[^\p{L}\p{N}_])_(\S(?:.*?\S)?)_(?![\p{L}\p{N}_])/gu, "$1$2")
      .replace(/\\\s*$/, "")
      .trim(),
  );
}

/**
 * The first line is the title (trimmed to 80 characters, without its markdown
 * marks); the rest is the description, as markdown.
 */
export function splitTaskText(text: string): { title: string; body: string } {
  const trimmed = text.replace(/^\s+/, "");
  const newline = trimmed.indexOf("\n");
  const line = (newline === -1 ? trimmed : trimmed.slice(0, newline)).trim();
  const first = plainTitle(line);
  const rest = newline === -1 ? "" : trimmed.slice(newline + 1).trim();
  if (first.length <= MAX_TITLE) return { title: first, body: rest };
  // A long first line keeps its full text in the description.
  return { title: `${first.slice(0, MAX_TITLE - 1).trimEnd()}…`, body: [line, rest].filter(Boolean).join("\n\n") };
}

const NO_PROJECT = "No project · no checkout";
const NO_MILESTONE = "No milestone";

function projectLabel(project: Project): string {
  return project.prefix ? `${project.prefix} · ${project.name}` : project.name;
}

/** A pill's select is as wide as the option it shows, like the prototype's `ntFit`. */
function fit(label: string) {
  return { width: `calc(${label.length}ch + 22px)` };
}

/**
 * The New task sheet: one formatted text box, a few pills on the right, files
 * dropped on the text box. It drops from the top of the window like the other
 * sheets.
 *
 * A task can belong to no project. Such a task has no repository, and its runs
 * work without a checkout.
 */
export default function NewTaskDialog({
  workspaceId,
  projects,
  agents,
  milestones,
  defaultProjectId,
  onCreated,
  onClose,
}: {
  workspaceId: string;
  projects: Project[];
  agents: Agent[];
  /** Open milestones are offered; the default is none. */
  milestones: Milestone[];
  defaultProjectId: string | null;
  /** Called after each task is created (and its files uploaded). */
  onCreated: (issue: IssueSummary) => void | Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const area = useRef<HTMLDivElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const [text, setText] = useState("");
  const [projectId, setProjectId] = useState(defaultProjectId ?? projects[0]?.id ?? "");
  const [priority, setPriority] = useState("medium");
  const [assignee, setAssignee] = useState("");
  const [milestoneId, setMilestoneId] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<string[]>([]);
  const pending = useRef<IssueSummary | null>(null);
  // A click closes the sheet only when it both started and ended on the
  // backdrop: a text selection dragged out of the box ends there too.
  const backdrop = useRef({ down: false, up: false });

  /** The editor's own editable element, inside RichText. */
  const focusText = () => area.current?.querySelector<HTMLElement>("[contenteditable]")?.focus();

  useEffect(() => {
    dialog.current?.showModal();
    return () => dialog.current?.close();
  }, []);

  const { title, body } = splitTaskText(text);
  // A milestone of another project is left out; one of no project fits any task.
  const milestoneChoices = milestones.filter(
    (entry) => entry.status === "open" && (!entry.projectId || entry.projectId === projectId),
  );
  const milestone = milestoneChoices.find((entry) => entry.id === milestoneId) ?? null;
  const project = projects.find((entry) => entry.id === projectId) ?? null;
  const agent = assignee.startsWith("agent:") ? (agents.find((entry) => `agent:${entry.id}` === assignee) ?? null) : null;
  const assigneeName = agent?.name ?? (assignee ? "You" : "Unassigned");

  /** Close, after asking when that would throw away typed text or picked files. */
  function dismiss() {
    if (busy) return;
    if ((text.trim() || files.length) && !window.confirm("Discard this task? What you typed and the files you added will be lost.")) {
      focusText();
      return;
    }
    onClose();
  }

  async function create(another: boolean) {
    if (busy) return;
    const fallbackTitle = files[0]?.name ?? "";
    if (!title && !fallbackTitle) {
      setError("Describe the task or attach a file so people and agents know what to do.");
      focusText();
      return;
    }
    setBusy(true);
    setError(null);
    // A task created before an upload failed is kept, so a retry only sends
    // the files that are left instead of filing the task twice.
    let remaining = files;
    try {
      let issue = pending.current;
      if (!issue) {
        // An empty choice is "No project": the task gets no repository.
        issue = await createIssue({
          workspaceId,
          projectId: project?.id ?? null,
          repoId: project?.repos[0]?.id ?? null,
          title: title || fallbackTitle,
          body,
          priority,
          dueDate: null,
          ...parseAssigneeKey(assignee),
          milestoneId: milestone?.id ?? null,
        });
        pending.current = issue;
      }
      for (const file of files) {
        await uploadArtifact({
          workspaceId,
          projectId: issue.projectId,
          milestoneId: null,
          issueId: issue.id,
          title: file.name,
          file,
        });
        remaining = remaining.filter((entry) => entry !== file);
      }
      pending.current = null;
      await onCreated(issue);
      if (another) {
        setCreated((list) => [...list, issue.key]);
        setText("");
        setFiles([]);
        setBusy(false);
        focusText();
      } else {
        onClose();
      }
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setFiles(remaining);
      setError(
        pending.current
          ? `${pending.current.key} was created, but a file did not upload: ${message}. Create again to retry the files that are left.`
          : message,
      );
      setBusy(false);
    }
  }

  const onKeyDown = (event: KeyboardEvent) => {
    // The editor handles its own ⌘↵ and marks the event handled.
    if (event.defaultPrevented) return;
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      void create(event.shiftKey);
    }
  };

  const addFiles = (list: FileList | null) => {
    if (!list?.length) return;
    const picked = Array.from(list);
    setFiles((current) => [...current, ...picked]);
  };

  const carriesFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files");

  return (
    <dialog
      ref={dialog}
      aria-labelledby="new-task-title"
      className="settings-dialog new-task-dialog"
      onCancel={(event) => {
        event.preventDefault();
        dismiss();
      }}
      onMouseDown={(event) => {
        backdrop.current = { down: event.target === event.currentTarget, up: false };
      }}
      onMouseUp={(event) => {
        backdrop.current.up = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        const { down, up } = backdrop.current;
        backdrop.current = { down: false, up: false };
        if (down && up && event.target === event.currentTarget) dismiss();
      }}
      onKeyDown={onKeyDown}
      // Files are taken only on the text box; dropped anywhere else in the
      // sheet they are refused instead of opened by the web view.
      onDragOver={(event) => {
        if (!carriesFiles(event)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "none";
      }}
      onDrop={(event) => {
        if (carriesFiles(event)) event.preventDefault();
      }}
    >
      <form
        className="settings-form"
        onSubmit={(event) => {
          event.preventDefault();
          void create(false);
        }}
      >
        <div className="settings-head">
          <h2 id="new-task-title">New task</h2>
          <button type="button" className="icon-btn" aria-label="Close" onClick={dismiss} disabled={busy}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <div className="settings-content">
          <div
            ref={area}
            className={`new-task-text${over ? " over" : ""}`}
            onDragOver={(event) => {
              if (!carriesFiles(event)) return;
              event.preventDefault();
              event.stopPropagation();
              event.dataTransfer.dropEffect = "copy";
              setOver(true);
            }}
            onDragLeave={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(false);
            }}
            onDrop={(event) => {
              setOver(false);
              if (!event.dataTransfer.files.length) return;
              event.preventDefault();
              event.stopPropagation();
              addFiles(event.dataTransfer.files);
            }}
          >
            <RichText
              value={text}
              ariaLabel="Describe the task"
              autoFocus
              minHeight={150}
              placeholder="What needs to be done? The first line becomes the title and anything after it is the description. You can drag and drop files here to attach them."
              onChange={(markdown) => {
                setText(markdown);
                setError(null);
              }}
              onSubmit={(another) => void create(another)}
            />
          </div>
          {/* The keyboard's way to attach files, beside dropping them on the text box. */}
          <button type="button" className="link new-task-choose" onClick={() => picker.current?.click()} disabled={busy}>
            or choose files to attach
          </button>
          <input
            ref={picker}
            type="file"
            multiple
            hidden
            tabIndex={-1}
            aria-hidden="true"
            onChange={(event) => {
              addFiles(event.target.files);
              // Cleared, so picking the same file again still reports a change.
              event.target.value = "";
            }}
          />
          {title ? (
            <p className="new-task-preview">
              <span className="dim">Title:</span> {title}
            </p>
          ) : null}
          {error ? (
            <p className="error" role="alert">
              {error}
            </p>
          ) : null}
          {files.length ? (
            <ul className="new-task-files" aria-label="Files to attach">
              {files.map((file, index) => (
                <li key={`${file.name}-${index}`}>
                  <Icon name="clip" size={12} />
                  <span className="truncate">{file.name}</span>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Remove ${file.name}`}
                    onClick={() => setFiles((list) => list.filter((_, at) => at !== index))}
                  >
                    <Icon name="close" size={12} />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
          <div className="new-task-pills">
            <label className="pill nt-pill">
              <span className="pill-ic" aria-hidden="true">
                <Icon name="folder" size={13} />
              </span>
              <select
                aria-label="Project"
                value={projectId}
                style={fit(project ? projectLabel(project) : NO_PROJECT)}
                onChange={(event) => {
                  setProjectId(event.target.value);
                  setMilestoneId("");
                }}
              >
                {projects.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {projectLabel(entry)}
                  </option>
                ))}
                <option value="">{NO_PROJECT}</option>
              </select>
            </label>
            <label className="pill nt-pill">
              <span className="pill-ic" aria-hidden="true">
                <PriorityIcon priority={priority} />
              </span>
              <select aria-label="Priority" value={priority} style={fit(priorityLabel(priority))} onChange={(event) => setPriority(event.target.value)}>
                <PriorityOptions />
              </select>
            </label>
            <label className="pill nt-pill">
              <span className="pill-ic" aria-hidden="true">
                <Icon name="flag" size={13} />
              </span>
              <select
                aria-label="Milestone"
                value={milestone?.id ?? ""}
                style={fit(milestone?.title ?? NO_MILESTONE)}
                onChange={(event) => setMilestoneId(event.target.value)}
              >
                <option value="">{NO_MILESTONE}</option>
                {milestoneChoices.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.title}
                  </option>
                ))}
              </select>
            </label>
            <label className="pill nt-pill">
              <span className="pill-ic" aria-hidden="true">
                <Avatar name={assigneeName} kind={agent ? "agent" : "human"} size="sm" />
              </span>
              <select aria-label="Assign to" value={assignee} style={fit(assigneeName)} onChange={(event) => setAssignee(event.target.value)}>
                <AssigneeOptions agents={agents} />
              </select>
            </label>
          </div>
        </div>
        <div className="settings-actions">
          {created.length ? (
            <span className="new-task-created" role="status">
              Created {created.map((key) => (
                <span key={key} className="chip mono">
                  {key}
                </span>
              ))}
            </span>
          ) : null}
          <button type="button" className="ghost" onClick={dismiss} disabled={busy}>
            Cancel
          </button>
          <button type="button" className="secondary" onClick={() => void create(true)} disabled={busy} title="⌘⇧↵">
            Create and add another
          </button>
          <button type="submit" className="primary" disabled={busy} title="⌘↵">
            {busy ? "Creating…" : "Create"}
          </button>
        </div>
      </form>
    </dialog>
  );
}
