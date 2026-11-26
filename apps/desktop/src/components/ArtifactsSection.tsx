import type { Artifact } from "../api";
import { typeLabel } from "../artifactBits";
import Icon from "../Icon";
import { fmtWhen } from "./TaskBits";

const typeTone = (type: string) => (type === "plan" ? "info" : type === "report" ? "ok" : "");

/** A task's artifacts as compact rows (`.art-mini`), with New opening the shared dialog. */
export default function ArtifactsSection({
  artifacts,
  onNew,
  onOpen,
}: {
  artifacts: Artifact[];
  onNew: () => void;
  onOpen: (id: string) => void;
}) {
  return (
    <section className="field" aria-label="Artifacts">
      <div className="field-label art-section">
        Artifacts · {artifacts.length}
        <button type="button" className="ghost small" onClick={onNew}>
          New
        </button>
      </div>
      {artifacts.map((artifact) => (
        <button key={artifact.id} type="button" className="art-mini" onClick={() => onOpen(artifact.id)}>
          <Icon name="file" size={12} />
          <span className="art-mini-title">{artifact.title}</span>
          {artifact.files.length > 0 && (
            <span className="count">
              <Icon name="clip" size={11} /> {artifact.files.length}
            </span>
          )}
          <span className={`chip ${typeTone(artifact.type)}`}>{typeLabel(artifact.type)}</span>
          <span className="muted">{fmtWhen(artifact.updatedAt)}</span>
        </button>
      ))}
    </section>
  );
}
