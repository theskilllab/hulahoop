import { useRef, useState } from "react";
import { downloadArtifact, gql, uploadArtifact, type Artifact, type Milestone, type Project } from "../api";

interface Props {
  workspaceId: string;
  projects: Project[];
  milestones: Milestone[];
  artifacts: Artifact[];
  onChanged: () => Promise<void> | void;
}

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/**
 * The shelf. Bytes live under `artifacts/<id>/<filename>` on the server; the row is
 * the index, and the download is an authenticated read rather than a public URL.
 */
export default function Artifacts({ workspaceId, projects, milestones, artifacts, onChanged }: Props) {
  const fileInput = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [title, setTitle] = useState("");
  const [milestoneId, setMilestoneId] = useState<string>("");
  const [projectId, setProjectId] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  async function upload() {
    if (!file) {
      setError("pick a file first");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await uploadArtifact({
        workspaceId,
        projectId: projectId || null,
        milestoneId: milestoneId || null,
        title: title.trim(),
        file,
      });
      setFile(null);
      setTitle("");
      if (fileInput.current) fileInput.current.value = "";
      await onChanged();
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: string) {
    await gql(`mutation ($id: UUID!) { deleteArtifact(id: $id) }`, { id });
    await onChanged();
  }

  return (
    <section className="screen page">
      <header className="sheet-head">
        <div>
          <h1>Artifacts</h1>
          <p className="hint">
            Reference files: a spec, a screenshot, a tarball. Attach one to a milestone or leave it
            loose.
          </p>
        </div>
        <button className="primary" onClick={() => setCreating(!creating)} aria-expanded={creating}>
          {creating ? "Cancel" : "Add a file"}
        </button>
      </header>

      {creating ? (
      <div className="panel">
        <h2>Add a file</h2>
        <div className="grid-2">
          <label>
            <span>file</span>
            <input
              ref={fileInput}
              type="file"
              onChange={(event) => {
                const picked = event.target.files?.[0] ?? null;
                setFile(picked);
                if (picked && !title) setTitle(picked.name.replace(/\.[^.]+$/, ""));
              }}
            />
          </label>
          <label>
            <span>title (optional)</span>
            <input value={title} onChange={(event) => setTitle(event.target.value)} placeholder="Spec, v2" />
          </label>
          <label>
            <span>milestone (optional)</span>
            <select value={milestoneId} onChange={(event) => setMilestoneId(event.target.value)}>
              <option value="">unfiled</option>
              {milestones.map((milestone) => (
                <option key={milestone.id} value={milestone.id}>
                  {milestone.title}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>project (optional)</span>
            <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
              <option value="">workspace</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.prefix ? `${project.prefix} · ${project.name}` : project.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        {error && <p className="error">{error}</p>}
        <div className="row">
          <button
            className="primary"
            onClick={() => void upload().then(() => setCreating(false))}
            disabled={busy || !file}
          >
            {busy ? "uploading…" : "upload"}
          </button>
          <button className="ghost" onClick={() => setCreating(false)}>
            Cancel
          </button>
        </div>
      </div>
      ) : null}

      {artifacts.length === 0 && <p className="hint">Nothing on the shelf yet.</p>}

      <table className="table">
        {artifacts.length > 0 && (
          <thead>
            <tr>
              <th>title</th>
              <th>file</th>
              <th>milestone</th>
              <th>size</th>
              <th />
            </tr>
          </thead>
        )}
        <tbody>
          {artifacts.map((artifact) => {
            const milestone = milestones.find((entry) => entry.id === artifact.milestoneId);
            return (
              <tr key={artifact.id}>
                <td>{artifact.title}</td>
                <td>
                  <button className="link" onClick={() => void downloadArtifact(artifact)}>
                    {artifact.filename}
                  </button>{" "}
                  <span className="dim">{artifact.kind}</span>
                </td>
                <td>{milestone ? milestone.title : <span className="dim">unfiled</span>}</td>
                <td className="dim">{size(artifact.size)}</td>
                <td>
                  <button className="link danger" onClick={() => void remove(artifact.id)}>
                    delete
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}
