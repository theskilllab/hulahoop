import { useEffect, useRef, useState } from "react";
import { addArtifactFiles, downloadArtifact, moveArtifact, removeArtifactFile, updateArtifact, type Artifact, type IssueSummary, type Milestone, type Project } from "../api";
import { formatSize, MAX_FILE_BYTES, ownerIds, ownerOf, ownerOptions, typeLabel } from "../artifactBits";
import { Preview, previewKind } from "../components/FilesSection";
import MarkdownView from "../components/MarkdownView";
import Select from "../components/Select";
import { StatusIcon } from "../components/TaskBits";
import Icon from "../Icon";

const when = (iso: string) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
const statusLabel = (status: string) => status.replace(/_/g, " ");

/** The artifact's own page: a breadcrumb, its content and files, and a side panel of facts. */
export default function ArtifactDetail({
  artifact,
  projects,
  milestones,
  issues,
  onChanged,
  onOpenIssue,
  onOpenMilestone,
  onAskDelete,
  onToast,
}: {
  artifact: Artifact;
  projects: Project[];
  milestones: Milestone[];
  issues: IssueSummary[];
  onChanged: () => Promise<void> | void;
  onOpenIssue: (id: string) => void;
  onOpenMilestone: () => void;
  onAskDelete: () => void;
  onToast: (text: string, bad?: boolean) => void;
}) {
  const picker = useRef<HTMLInputElement>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(artifact.content);
  const [open, setOpen] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  useEffect(() => setDraft(artifact.content), [artifact.id, artifact.content]);

  const issue = issues.find((entry) => entry.id === artifact.issueId);
  const milestone = milestones.find((entry) => entry.id === artifact.milestoneId);
  const project = projects.find((entry) => entry.id === artifact.projectId);

  const attempt = async (work: () => Promise<void>) => {
    try {
      await work();
      await onChanged();
    } catch (cause) {
      onToast(String(cause instanceof Error ? cause.message : cause), true);
    }
  };

  const addFiles = (list: FileList | File[]) => {
    const picked = Array.from(list).filter((file) => {
      if (file.size <= MAX_FILE_BYTES) return true;
      onToast(`${file.name} is over 25 MB and was not added`, true);
      return false;
    });
    if (picked.length) void attempt(() => addArtifactFiles(artifact.id, picked));
  };

  return (
    <div className="art-detail">
      <div className="art-main">
        <nav className="crumb" aria-label="Breadcrumb">
          {project && <span className="dot" style={{ background: project.color }} />}
          {project && <span>{project.name}</span>}
          <Icon name="chevron" size={10} />
          {issue ? (
            <button type="button" className="link" onClick={() => onOpenIssue(issue.id)}>
              {issue.key}
            </button>
          ) : null}
          {issue ? <span>{issue.title}</span> : null}
          {milestone ? (
            <button type="button" className="link" onClick={onOpenMilestone}>
              Milestone: {milestone.title}
            </button>
          ) : null}
        </nav>
        <h1 className="art-title">{artifact.title}</h1>

        <div className="field-label art-section">
          Content
          <button
            type="button"
            className="ghost small"
            onClick={() => {
              if (editing && draft !== artifact.content) void attempt(() => updateArtifact(artifact.id, { content: draft }));
              setEditing(!editing);
            }}
          >
            {editing ? "Done" : "Edit"}
          </button>
        </div>
        {editing ? (
          <textarea className="mono" rows={12} aria-label="Content" value={draft} onChange={(event) => setDraft(event.target.value)} />
        ) : (
          <MarkdownView value={artifact.content} empty="No content yet." />
        )}

        <div className="field-label art-section">
          Files · {artifact.files.length}
          <button type="button" className="ghost small" onClick={() => picker.current?.click()}>
            Add files
          </button>
          <input ref={picker} type="file" multiple hidden onChange={(event) => event.target.files && addFiles(event.target.files)} />
        </div>
        <ul className="art-files">
          {artifact.files.map((file) => (
            <li key={file.id}>
              <div className="art-file-row">
                <button type="button" className="mt-main" aria-expanded={open === file.id} onClick={() => setOpen(open === file.id ? null : file.id)}>
                  <Icon name="clip" size={12} />
                  <span className="truncate">{file.filename}</span>
                  <span className="muted">{formatSize(file.size)}</span>
                </button>
                <button type="button" className="icon-btn" aria-label={`Download ${file.filename}`} onClick={() => void downloadArtifact(file).catch((cause) => onToast(String(cause), true))}>
                  <Icon name="download" size={12} />
                </button>
                <button type="button" className="icon-btn" aria-label={`Remove ${file.filename}`} onClick={() => void attempt(() => removeArtifactFile(file.id))}>
                  <Icon name="close" size={12} />
                </button>
              </div>
              {open === file.id && (previewKind(file) ? <Preview file={file} /> : <p className="muted">No preview for this file type.</p>)}
            </li>
          ))}
        </ul>
        <div
          className={`file-drop${over ? " over" : ""}`}
          onDragOver={(event) => {
            event.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(event) => {
            event.preventDefault();
            setOver(false);
            addFiles(event.dataTransfer.files);
          }}
        >
          Drop files here
        </div>
      </div>

      <aside className="art-side" aria-label="Details">
        <dl>
          <dt>Type</dt>
          <dd>{typeLabel(artifact.type)}</dd>
          <dt>Belongs to</dt>
          <dd>
            <Select
              label="Belongs to"
              value={ownerOf(artifact)}
              options={ownerOptions(issues, milestones)}
              onChange={(next) => void attempt(() => moveArtifact(artifact.id, ownerIds(next)))}
            />
          </dd>
          <dt>Project</dt>
          <dd>{project?.name ?? "All projects"}</dd>
          {issue && (
            <>
              <dt>Task status</dt>
              <dd>
                <StatusIcon status={issue.status} size={14} /> {statusLabel(issue.status)}
              </dd>
            </>
          )}
          {milestone && (
            <>
              <dt>Milestone</dt>
              <dd>{milestone.title}</dd>
            </>
          )}
          <dt>Author</dt>
          <dd>{artifact.author ? `${artifact.author}${artifact.authorIsAgent ? " · agent" : ""}` : "Unknown"}</dd>
          <dt>Created</dt>
          <dd>{when(artifact.createdAt)}</dd>
          <dt>Updated</dt>
          <dd>{when(artifact.updatedAt)}</dd>
        </dl>
        {issue && (
          <button type="button" className="ghost" onClick={() => onOpenIssue(issue.id)}>
            Open task →
          </button>
        )}
        {milestone && (
          <button type="button" className="ghost" onClick={onOpenMilestone}>
            Open milestone →
          </button>
        )}
        <button type="button" className="danger" onClick={onAskDelete}>
          Delete artifact
        </button>
      </aside>
    </div>
  );
}
