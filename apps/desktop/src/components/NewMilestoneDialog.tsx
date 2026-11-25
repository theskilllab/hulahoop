import { useEffect, useId, useRef, useState, type DragEvent } from "react";
import { gql, uploadArtifact, type Project } from "../api";
import Icon from "../Icon";

const MIN_NAME = 3;

function projectLabel(project: Project): string {
  return project.prefix ? `${project.prefix} · ${project.name}` : project.name;
}

const carriesFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files");

/**
 * The New milestone sheet: a name, a due date, a project and a description.
 * Files dropped on the description are attached to the milestone as artifacts.
 * It drops from the top of the window like New task.
 */
export default function NewMilestoneDialog({
  workspaceId,
  projects,
  onCreated,
  onClose,
}: {
  workspaceId: string;
  projects: Project[];
  /** Called once the milestone exists and its files are uploaded. */
  onCreated: (milestone: { id: string; title: string }) => void | Promise<void>;
  onClose: () => void;
}) {
  const ids = { name: useId(), due: useId(), project: useId(), description: useId() };
  const dialog = useRef<HTMLDialogElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [dueDate, setDueDate] = useState("");
  const [projectId, setProjectId] = useState("");
  const [description, setDescription] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A milestone created before an upload failed is kept, so a retry only sends
  // the files that are left instead of creating the milestone twice.
  const pending = useRef<{ id: string; title: string } | null>(null);
  // A click closes the sheet only when it both started and ended on the backdrop.
  const backdrop = useRef({ down: false, up: false });

  useEffect(() => {
    dialog.current?.showModal();
    return () => dialog.current?.close();
  }, []);

  /** Close, after asking when that would throw away typed text or picked files. */
  function dismiss() {
    if (busy) return;
    const typed = name.trim() || description.trim() || dueDate || files.length;
    if (typed && !window.confirm("Discard this milestone? What you typed and the files you added will be lost.")) return;
    onClose();
  }

  async function create() {
    if (busy) return;
    const title = name.trim();
    if (!pending.current && title.length < MIN_NAME) {
      setError("Name the milestone so people know what it is working toward.");
      return;
    }
    setBusy(true);
    setError(null);
    let remaining = files;
    try {
      if (!pending.current) {
        const data = await gql<{ createMilestone: { id: string } }>(
          `mutation ($workspaceId: UUID!, $projectId: UUID, $title: String!, $body: String, $targetDate: String) {
            createMilestone(workspaceId: $workspaceId, projectId: $projectId, title: $title, body: $body, targetDate: $targetDate) { id }
          }`,
          { workspaceId, projectId: projectId || null, title, body: description, targetDate: dueDate || null },
        );
        pending.current = { id: data.createMilestone.id, title };
      }
      const created = pending.current;
      for (const file of files) {
        await uploadArtifact({
          workspaceId,
          projectId: projectId || null,
          milestoneId: created.id,
          issueId: null,
          title: file.name,
          file,
        });
        remaining = remaining.filter((entry) => entry !== file);
      }
      pending.current = null;
      await onCreated(created);
      onClose();
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setFiles(remaining);
      setError(
        pending.current
          ? `The milestone was created, but a file did not upload: ${message}. Create again to retry the files that are left.`
          : message,
      );
      setBusy(false);
    }
  }

  const addFiles = (list: FileList | null) => {
    if (!list?.length) return;
    const picked = Array.from(list);
    setFiles((current) => [...current, ...picked]);
  };

  const nameInvalid = error !== null && !pending.current && name.trim().length < MIN_NAME;

  return (
    <dialog
      ref={dialog}
      aria-labelledby="new-milestone-title"
      className="settings-dialog new-milestone-dialog"
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
      // Files are taken only on the description; dropped anywhere else in the
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
          void create();
        }}
      >
        <div className="settings-head">
          <h2 id="new-milestone-title">New milestone</h2>
          <button type="button" className="icon-btn" aria-label="Close" onClick={dismiss} disabled={busy}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <div className="settings-content">
          <div className="field">
            <label className="field-label" htmlFor={ids.name}>
              Name
            </label>
            <input
              id={ids.name}
              type="text"
              autoFocus
              value={name}
              placeholder="GitHub identity"
              aria-invalid={nameInvalid}
              onChange={(event) => {
                setName(event.target.value);
                setError(null);
              }}
            />
          </div>
          <div className="new-milestone-pair">
            <div className="field">
              <label className="field-label" htmlFor={ids.due}>
                Due date (optional)
              </label>
              <input id={ids.due} type="date" value={dueDate} onChange={(event) => setDueDate(event.target.value)} />
            </div>
            <div className="field">
              <label className="field-label" htmlFor={ids.project}>
                Project
              </label>
              <select id={ids.project} value={projectId} onChange={(event) => setProjectId(event.target.value)}>
                <option value="">No project</option>
                {projects.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {projectLabel(entry)}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="field">
            <label className="field-label" htmlFor={ids.description}>
              Description (optional)
            </label>
            <div
              className={`new-milestone-text${over ? " over" : ""}`}
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
              <textarea
                id={ids.description}
                rows={5}
                value={description}
                placeholder="What is this milestone working toward? You can drag and drop files here to attach them."
                onChange={(event) => setDescription(event.target.value)}
              />
            </div>
            {/* The keyboard's way to attach files, beside dropping them on the description. */}
            <button type="button" className="link new-milestone-choose" onClick={() => picker.current?.click()} disabled={busy}>
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
          </div>
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
        </div>
        <div className="settings-actions">
          <button type="button" className="ghost" onClick={dismiss} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={busy}>
            {busy ? "Creating…" : "Create milestone"}
          </button>
        </div>
      </form>
    </dialog>
  );
}
