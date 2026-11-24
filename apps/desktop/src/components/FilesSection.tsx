import { useEffect, useRef, useState } from "react";
import { deleteArtifact, downloadArtifact, fetchArtifactBlob, uploadArtifact, type Artifact } from "../api";
import Icon from "../Icon";
import { fmtWhen } from "./TaskBits";

function fmtSize(bytes: number): string {
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

const TEXT_MIME = /^(text\/|application\/(json|xml|x-yaml|yaml|toml|javascript|typescript))/;
const TEXT_EXT = /\.(md|txt|json|ya?ml|toml|csv|log|ts|tsx|js|rs|py|go|sh|sql|html|css)$/i;

function previewKind(file: Artifact): "image" | "text" | null {
  if (file.mime?.startsWith("image/")) return "image";
  if ((file.mime && TEXT_MIME.test(file.mime)) || TEXT_EXT.test(file.filename)) return "text";
  return null;
}

/** An image or the first part of a text file, fetched when the row opens. */
function Preview({ file }: { file: Artifact }) {
  const kind = previewKind(file);
  const [url, setUrl] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let objectUrl: string | null = null;
    fetchArtifactBlob(file)
      .then(async (blob) => {
        if (!alive) return;
        if (kind === "image") {
          objectUrl = URL.createObjectURL(blob);
          setUrl(objectUrl);
        } else {
          const content = await blob.slice(0, 64 * 1024).text();
          if (alive) setText(content);
        }
      })
      .catch((cause) => alive && setError(cause instanceof Error ? cause.message : String(cause)));
    return () => {
      alive = false;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [file.id, kind]);

  if (error) return <p className="error">{error}</p>;
  if (kind === "image") return url ? <img className="file-preview-img" src={url} alt={file.title || file.filename} /> : <p className="dim small">Loading…</p>;
  return text === null ? <p className="dim small">Loading…</p> : <pre className="file-preview-text">{text}</pre>;
}

/**
 * A task's files: the artifacts linked to it. Add by button or by dropping on
 * the section, preview images and text inline, download or remove.
 */
export default function FilesSection({
  files,
  workspaceId,
  projectId,
  issueId,
  onChanged,
  onError,
}: {
  files: Artifact[];
  workspaceId: string;
  projectId: string | null;
  issueId: string;
  onChanged: () => void | Promise<void>;
  onError: (message: string) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const [busy, setBusy] = useState(false);

  async function add(list: FileList | File[] | null) {
    const picked = list ? Array.from(list) : [];
    if (!picked.length) return;
    setBusy(true);
    try {
      for (const file of picked) {
        await uploadArtifact({ workspaceId, projectId, milestoneId: null, issueId, title: file.name, file });
      }
      await onChanged();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove(file: Artifact) {
    if (!window.confirm(`Remove ${file.filename} from this task?`)) return;
    try {
      await deleteArtifact(file.id);
      await onChanged();
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <section
      className={`tsec files${over ? " over" : ""}`}
      aria-labelledby="files-head"
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
        void add(event.dataTransfer.files);
      }}
    >
      <div className="tsec-head">
        <h3 id="files-head">
          Files <span className="dim">· {files.length}</span>
        </h3>
        <button type="button" className="ghost small-btn" onClick={() => input.current?.click()} disabled={busy}>
          <Icon name="plus" size={12} />
          {busy ? "Uploading…" : "Add files"}
        </button>
        <input ref={input} type="file" multiple hidden onChange={(event) => {
            void add(event.target.files);
            event.target.value = "";
          }}
        />
      </div>
      {files.length ? (
        <ul className="file-list">
          {files.map((file) => {
            const previewable = previewKind(file) !== null;
            const expanded = open === file.id;
            return (
              <li key={file.id} className={expanded ? "open" : undefined}>
                <div className="file-row">
                  <button
                    type="button"
                    className="file-name"
                    aria-expanded={previewable ? expanded : undefined}
                    disabled={!previewable}
                    title={previewable ? (expanded ? "Hide preview" : "Show preview") : file.filename}
                    onClick={() => setOpen(expanded ? null : file.id)}
                  >
                    <Icon name={previewKind(file) === "image" ? "file" : "clip"} size={13} />
                    <span className="truncate">{file.title || file.filename}</span>
                  </button>
                  <span className="dim small">
                    {fmtSize(file.size)} · {fmtWhen(file.createdAt)}
                  </span>
                  <button type="button" className="icon-btn" aria-label={`Download ${file.filename}`} title="Download" onClick={() => void downloadArtifact(file).catch((cause) => onError(String(cause)))}>
                    <Icon name="download" size={13} />
                  </button>
                  <button type="button" className="icon-btn" aria-label={`Remove ${file.filename}`} title="Remove" onClick={() => void remove(file)}>
                    <Icon name="trash" size={13} />
                  </button>
                </div>
                {expanded ? (
                  <div className="file-preview">
                    <Preview file={file} />
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      <p className="file-drop">Drop files here, or use Add files</p>
    </section>
  );
}
