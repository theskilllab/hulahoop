import { useEffect, useId, useRef, useState, type DragEvent, type FormEvent } from "react";
import { createArtifact, PartialArtifactError, type Artifact, type IssueSummary, type Milestone, type Project } from "../api";
import { ARTIFACT_TYPES, formatSize, MAX_FILE_BYTES, ownerIds, ownerOptions } from "../artifactBits";
import Icon from "../Icon";
import Select from "./Select";

const PLAN_TEMPLATE = "## Goal\n\n## Steps\n\n## Risks\n";
const carriesFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files");

/**
 * The one New artifact dialog every entry point opens. Files are dragged in; a
 * title or at least one file is required. `preset` fixes what it belongs to.
 */
export default function NewArtifactDialog({
  workspaceId,
  issues,
  milestones,
  preset,
  onCreated,
  onClose,
  onToast,
}: {
  workspaceId: string;
  projects: Project[];
  milestones: Milestone[];
  issues: IssueSummary[];
  preset?: { issueId?: string; milestoneId?: string };
  onCreated: (artifact: Artifact) => void | Promise<void>;
  onClose: () => void;
  onToast: (text: string, bad?: boolean) => void;
}) {
  const ids = { title: useId(), content: useId() };
  const dialog = useRef<HTMLDialogElement>(null);
  const options = ownerOptions(issues, milestones);
  const [title, setTitle] = useState("");
  const [type, setType] = useState<Artifact["type"]>("plan");
  const [owner, setOwner] = useState(
    preset?.issueId ? `issue:${preset.issueId}` : preset?.milestoneId ? `milestone:${preset.milestoneId}` : (options[0]?.value ?? ""),
  );
  const [content, setContent] = useState("");
  const [files, setFiles] = useState<File[]>([]);
  const [over, setOver] = useState(false);
  const [invalid, setInvalid] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    dialog.current?.showModal();
    return () => dialog.current?.close();
  }, []);

  function take(list: FileList | File[]) {
    const accepted: File[] = [];
    for (const file of Array.from(list)) {
      if (file.size > MAX_FILE_BYTES) onToast(`${file.name} is over 25 MB and was not added`, true);
      else accepted.push(file);
    }
    if (!accepted.length) return;
    setFiles((current) => [...current, ...accepted]);
    if (type === "plan" && !content.trim()) setType("file");
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    const name = title.trim() || files[0]?.name.replace(/\.[^.]+$/, "") || "";
    if (!name) {
      setInvalid(true);
      return;
    }
    setBusy(true);
    try {
      const body = type === "plan" && !content.trim() && !files.length ? PLAN_TEMPLATE : content;
      const artifact = await createArtifact({ workspaceId, title: name, type, content: body, ...ownerIds(owner), files });
      const owned = options.find((option) => option.value === owner)?.label ?? "";
      onToast(`Saved to Artifacts, ${owned.split(" · ")[0]}`, false);
      await onCreated(artifact);
      onClose();
    } catch (cause) {
      // The artifact exists already; retrying here would make a second one.
      // Open it so the missing files can be added from there.
      if (cause instanceof PartialArtifactError) {
        onToast(`Saved, but some files failed to upload: ${cause.message}`, true);
        await onCreated(cause.artifact);
        onClose();
        return;
      }
      onToast(String(cause instanceof Error ? cause.message : cause), true);
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialog}
      aria-labelledby="new-artifact-title"
      className={`settings-dialog new-artifact-dialog${over ? " drop" : ""}`}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onDragOver={(event) => {
        if (!carriesFiles(event)) return;
        event.preventDefault();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(event) => {
        if (!carriesFiles(event)) return;
        event.preventDefault();
        setOver(false);
        take(event.dataTransfer.files);
      }}
    >
      <form className="settings-form" onSubmit={(event) => void submit(event)}>
        <div className="settings-head">
          <h2 id="new-artifact-title">New artifact</h2>
        </div>
        <div className="settings-content">
          <div className="field">
            <label className="field-label" htmlFor={ids.title}>
              Title <span aria-hidden="true">*</span>
            </label>
            <input
              id={ids.title}
              type="text"
              autoFocus
              value={title}
              placeholder="Release plan"
              aria-invalid={invalid}
              onChange={(event) => {
                setTitle(event.target.value);
                setInvalid(false);
              }}
            />
            {invalid ? <p className="error" role="alert">Give the artifact a title, or attach a file.</p> : null}
          </div>
          <div className="new-milestone-pair">
            <div className="field">
              <span className="field-label">Type</span>
              <Select label="Type" value={type} options={ARTIFACT_TYPES} onChange={(next) => setType(next as Artifact["type"])} />
            </div>
            <div className="field">
              <span className="field-label">
                Belongs to <span aria-hidden="true">*</span>
              </span>
              <Select label="Belongs to" value={owner} options={options} onChange={setOwner} />
            </div>
          </div>
          <div className="field">
            <label className="field-label" htmlFor={ids.content}>
              Content (Markdown, optional)
            </label>
            <textarea
              id={ids.content}
              rows={5}
              value={content}
              placeholder="Leave empty to start from a template. You can drag and drop files anywhere in this window to attach them."
              onChange={(event) => setContent(event.target.value)}
            />
          </div>
          {files.length ? (
            <ul className="new-task-files" aria-label="Files to attach">
              {files.map((file, index) => (
                <li key={`${file.name}-${index}`}>
                  <Icon name="clip" size={12} />
                  <span className="truncate">{file.name}</span>
                  <span className="muted">{formatSize(file.size)}</span>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Remove ${file.name}`}
                    onClick={() => setFiles((current) => current.filter((_, at) => at !== index))}
                  >
                    <Icon name="close" size={12} />
                  </button>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
        <div className="settings-actions">
          <button type="button" className="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={busy}>
            Create artifact
          </button>
        </div>
      </form>
    </dialog>
  );
}
