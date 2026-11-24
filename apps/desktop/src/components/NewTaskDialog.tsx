import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { createIssue, uploadArtifact, type Agent, type IssueSummary, type Project } from "../api";
import Icon from "../Icon";
import { AssigneeOptions, PriorityOptions, parseAssigneeKey } from "./TaskBits";

const MAX_TITLE = 80;

/** The first line is the title (trimmed to 80 characters); the rest is the description. */
export function splitTaskText(text: string): { title: string; body: string } {
  const trimmed = text.replace(/^\s+/, "");
  const newline = trimmed.indexOf("\n");
  const first = (newline === -1 ? trimmed : trimmed.slice(0, newline)).trim();
  const rest = newline === -1 ? "" : trimmed.slice(newline + 1).trim();
  if (first.length <= MAX_TITLE) return { title: first, body: rest };
  // A long first line keeps its full text in the description.
  return { title: `${first.slice(0, MAX_TITLE - 1).trimEnd()}…`, body: [first, rest].filter(Boolean).join("\n\n") };
}

/**
 * The New task sheet: one text box, a few pills, files by drag and drop. It
 * drops from the top of the window like the other sheets.
 *
 * A task can belong to no project. Such a task has no repository, and its runs
 * work without a checkout.
 */
export default function NewTaskDialog({
  workspaceId,
  projects,
  agents,
  defaultProjectId,
  onCreated,
  onClose,
}: {
  workspaceId: string;
  projects: Project[];
  agents: Agent[];
  defaultProjectId: string | null;
  /** Called after each task is created (and its files uploaded). */
  onCreated: (issue: IssueSummary) => void | Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("");
  const [projectId, setProjectId] = useState(defaultProjectId ?? projects[0]?.id ?? "");
  const [priority, setPriority] = useState("medium");
  const [dueDate, setDueDate] = useState("");
  const [assignee, setAssignee] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<string[]>([]);
  const pending = useRef<IssueSummary | null>(null);
  // A click closes the sheet only when it both started and ended on the
  // backdrop: a text selection dragged out of the box ends there too.
  const backdrop = useRef({ down: false, up: false });

  useEffect(() => {
    dialog.current?.showModal();
    area.current?.focus();
    return () => dialog.current?.close();
  }, []);

  const { title, body } = splitTaskText(text);

  /** Close, after asking when that would throw away typed text or picked files. */
  function dismiss() {
    if (busy) return;
    if ((text.trim() || files.length) && !window.confirm("Discard this task? What you typed and the files you added will be lost.")) {
      area.current?.focus();
      return;
    }
    onClose();
  }

  async function create(another: boolean) {
    if (busy) return;
    const fallbackTitle = files[0]?.name ?? "";
    if (!title && !fallbackTitle) {
      setError("Describe the task or attach a file so people and agents know what to do.");
      area.current?.focus();
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
        const project = projects.find((entry) => entry.id === projectId) ?? null;
        issue = await createIssue({
          workspaceId,
          projectId: project?.id ?? null,
          repoId: project?.repos[0]?.id ?? null,
          title: title || fallbackTitle,
          body,
          priority,
          dueDate: dueDate || null,
          ...parseAssigneeKey(assignee),
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
        area.current?.focus();
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

  return (
    <dialog
      ref={dialog}
      aria-labelledby="new-task-title"
      className={`settings-dialog new-task-dialog${over ? " over" : ""}`}
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
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes("Files")) return;
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={(event) => {
        if (!event.dataTransfer.files.length) return;
        event.preventDefault();
        setOver(false);
        addFiles(event.dataTransfer.files);
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
          <textarea
            ref={area}
            className="new-task-text"
            rows={6}
            value={text}
            aria-label="Task"
            placeholder="What needs to be done? The first line becomes the title and anything after it is the description. Markdown works. You can drag and drop files here to attach them."
            onChange={(event) => {
              setText(event.target.value);
              if (error) setError(null);
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
            <label className="pill-field">
              <span>Project</span>
              <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
                {projects.map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.prefix ? `${project.prefix} · ` : ""}
                    {project.name}
                  </option>
                ))}
                <option value="">No project · no checkout</option>
              </select>
            </label>
            <label className="pill-field">
              <span>Priority</span>
              <select value={priority} onChange={(event) => setPriority(event.target.value)}>
                <PriorityOptions />
              </select>
            </label>
            <label className="pill-field">
              <span>Due</span>
              <input type="date" value={dueDate} onChange={(event) => setDueDate(event.target.value)} />
            </label>
            <label className="pill-field">
              <span>Assign to</span>
              <select value={assignee} onChange={(event) => setAssignee(event.target.value)}>
                <AssigneeOptions agents={agents} />
              </select>
            </label>
            <label className="pill-field file-pick">
              <Icon name="clip" size={12} />
              <span>Attach</span>
              <input type="file" multiple onChange={(event) => {
                  addFiles(event.target.files);
                  event.target.value = "";
                }}
              />
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
