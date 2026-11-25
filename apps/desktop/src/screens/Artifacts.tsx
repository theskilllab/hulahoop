import { useEffect, useMemo, useState } from "react";
import { deleteArtifact, type Artifact, type IssueSummary, type Milestone, type Project } from "../api";
import { ARTIFACT_TYPES, typeLabel } from "../artifactBits";
import ContextMenu from "../components/ContextMenu";
import NewArtifactDialog from "../components/NewArtifactDialog";
import Select from "../components/Select";
import { StatusIcon } from "../components/TaskBits";
import { useToast } from "../components/Toast";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import ArtifactDetail from "./ArtifactDetail";

interface Props {
  workspaceId: string;
  projects: Project[];
  milestones: Milestone[];
  issues: IssueSummary[];
  artifacts: Artifact[];
  /** An artifact to open on arrival, e.g. one just created from another screen. */
  initialOpenId?: string | null;
  onChanged: () => Promise<void> | void;
  onOpenIssue?: (id: string) => void;
  onOpenMilestones?: () => void;
}

const EMPTY_COPY = "Agents save their plans here automatically when they start a task. You can also add your own.";
const statusLabel = (status: string) => status.replace(/_/g, " ");
const typeTone = (type: string) => (type === "plan" ? "info" : type === "report" ? "ok" : "");
const shortDate = (iso: string) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });

/**
 * The shelf. Rows are the index; the bytes live on the server and are read with
 * the bearer token. New artifacts come from one modal dialog; delete asks first.
 */
export default function Artifacts({ workspaceId, projects, milestones, issues, artifacts, initialOpenId = null, onChanged, onOpenIssue, onOpenMilestones }: Props) {
  const [query, setQuery] = useState("");
  const [projectFilter, setProjectFilter] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [openId, setOpenId] = useState<string | null>(initialOpenId);
  const [creating, setCreating] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; artifact: Artifact } | null>(null);
  const [confirming, setConfirming] = useState<Artifact | null>(null);
  const [toast, showToast] = useToast();

  useEffect(() => setOpenId(initialOpenId), [initialOpenId]);
  useEffect(() => {
    if (!openId) return;
    const close = (event: KeyboardEvent) => event.key === "Escape" && !document.querySelector("dialog[open]") && setOpenId(null);
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [openId]);

  const issueOf = (artifact: Artifact) => issues.find((entry) => entry.id === artifact.issueId);
  const milestoneOf = (artifact: Artifact) => milestones.find((entry) => entry.id === artifact.milestoneId);
  const projectOf = (artifact: Artifact) => projects.find((entry) => entry.id === artifact.projectId);
  const ownerName = (artifact: Artifact) => issueOf(artifact)?.key ?? milestoneOf(artifact)?.title ?? "the workspace";

  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const haystack = (artifact: Artifact) => {
      const issue = issues.find((entry) => entry.id === artifact.issueId);
      const milestone = milestones.find((entry) => entry.id === artifact.milestoneId);
      return [artifact.title, issue?.key, issue?.title, milestone?.title, artifact.type, artifact.author, ...artifact.files.map((file) => file.filename)]
        .join("\n")
        .toLowerCase();
    };
    return artifacts
      .filter((artifact) => !projectFilter || artifact.projectId === projectFilter)
      .filter((artifact) => !typeFilter || artifact.type === typeFilter)
      .filter((artifact) => !needle || haystack(artifact).includes(needle))
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }, [artifacts, issues, milestones, query, projectFilter, typeFilter]);

  async function remove(artifact: Artifact) {
    try {
      await deleteArtifact(artifact.id);
      setConfirming(null);
      if (openId === artifact.id) setOpenId(null);
      await onChanged();
    } catch (cause) {
      showToast(String(cause instanceof Error ? cause.message : cause), true);
    }
  }

  const opened = artifacts.find((artifact) => artifact.id === openId);
  const alert = confirming && (
    <div className="review-bar" role="alert">
      <span className="od-fill">
        <strong>Delete this artifact?</strong> It will be removed from {ownerName(confirming)} and from Artifacts, along with {confirming.files.length} {confirming.files.length === 1 ? "file" : "files"}.
      </span>
      <button type="button" className="ghost" onClick={() => setConfirming(null)}>
        Cancel
      </button>
      <button type="button" className="danger" onClick={() => void remove(confirming)}>
        Delete
      </button>
    </div>
  );

  return (
    <section className="screen page">
      <PageHeader
        title="Artifacts"
        subtitle={`${artifacts.length} stored`}
        ownSearch
        actions={
          <>
            <input type="search" placeholder="Search artifacts" aria-label="Search artifacts" value={query} onChange={(event) => setQuery(event.target.value)} />
            <Select
              label="Project"
              value={projectFilter}
              options={[{ value: "", label: "All projects" }, ...projects.map((project) => ({ value: project.id, label: project.prefix ? `${project.prefix} · ${project.name}` : project.name }))]}
              onChange={setProjectFilter}
            />
            <Select
              label="Type"
              value={typeFilter}
              options={[{ value: "", label: "All types" }, ...ARTIFACT_TYPES.map((entry) => ({ value: entry.value, label: entry.plural }))]}
              onChange={setTypeFilter}
            />
            <button type="button" className="primary" onClick={() => setCreating(true)}>
              <Icon name="plus" size={14} /> New artifact
            </button>
          </>
        }
      />
      {alert}

      {opened ? (
        <ArtifactDetail
          artifact={opened}
          projects={projects}
          milestones={milestones}
          issues={issues}
          onChanged={onChanged}
          onOpenIssue={(id) => onOpenIssue?.(id)}
          onOpenMilestone={() => onOpenMilestones?.()}
          onAskDelete={() => setConfirming(opened)}
          onToast={showToast}
        />
      ) : shown.length === 0 ? (
        <div className="empty">
          <strong>{artifacts.length === 0 ? "No artifacts yet" : "No artifacts match these filters"}</strong>
          {EMPTY_COPY}
        </div>
      ) : (
        <div className="alist">
          <div className="arow head">
            <span>Artifact</span>
            <span>Belongs to</span>
            <span>Status</span>
            <span>Updated</span>
          </div>
          {shown.map((artifact) => {
            const issue = issueOf(artifact);
            const milestone = milestoneOf(artifact);
            const project = projectOf(artifact);
            return (
              <div
                key={artifact.id}
                className={`arow${artifact.id === openId ? " sel" : ""}`}
                tabIndex={0}
                onClick={() => setOpenId(artifact.id)}
                onKeyDown={(event) => event.key === "Enter" && setOpenId(artifact.id)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ x: event.clientX, y: event.clientY, artifact });
                }}
              >
                <div>
                  <div className="a-title">
                    {artifact.title}
                    {artifact.files.length > 0 && (
                      <span className="count" data-n={artifact.files.length} title={`${artifact.files.length} files`}>
                        <Icon name="clip" size={11} />
                      </span>
                    )}
                  </div>
                  <div className="a-sub">
                    <span className={`chip ${typeTone(artifact.type)}`}>{typeLabel(artifact.type)}</span>
                    {artifact.author && <span>{artifact.author}{artifact.authorIsAgent ? " · agent" : ""}</span>}
                  </div>
                </div>
                <div>
                  <div className="a-owner">
                    {project ? (
                      <>
                        <span className="dot" style={{ background: project.color }} /> {project.name}
                      </>
                    ) : (
                      "All projects"
                    )}
                  </div>
                  <div className="a-sub">
                    {issue ? (
                      <>
                        <span className="mono">{issue.key}</span> {issue.title}
                      </>
                    ) : milestone ? (
                      <>
                        <Icon name="flag" size={11} /> {milestone.title}
                      </>
                    ) : null}
                  </div>
                </div>
                <div className="a-status">
                  {issue ? (
                    <>
                      <StatusIcon status={issue.status} size={14} /> {statusLabel(issue.status)}
                    </>
                  ) : milestone ? (
                    <span className={`chip ${milestone.status === "done" ? "ok" : ""}`}>{statusLabel(milestone.status)}</span>
                  ) : null}
                </div>
                <div className="a-updated">{shortDate(artifact.updatedAt)}</div>
              </div>
            );
          })}
        </div>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label="Artifact actions"
          onClose={() => setMenu(null)}
          items={[
            { label: "Open", hint: "↵", onSelect: () => setOpenId(menu.artifact.id) },
            "separator",
            { label: "Delete…", danger: true, onSelect: () => setConfirming(menu.artifact) },
          ]}
        />
      )}

      {creating && (
        <NewArtifactDialog
          workspaceId={workspaceId}
          projects={projects}
          milestones={milestones}
          issues={issues}
          onToast={showToast}
          onCreated={async (artifact) => {
            await onChanged();
            setOpenId(artifact.id);
          }}
          onClose={() => setCreating(false)}
        />
      )}
      {toast}
    </section>
  );
}
