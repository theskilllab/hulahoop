import { useState } from "react";

import {
  loadRecentRuns,
  type Agent,
  type Artifact,
  type IssueSummary as Issue,
  type Milestone,
  type Project,
  type RecentRun,
} from "../api";
import Icon from "../Icon";
import PageHeader from "../PageHeader";
import ProjectDialog from "../components/ProjectDialog";
import { useToast } from "../components/Toast";
import { openMilestoneCount, projectStats } from "../projectStats";
import type { Workspace } from "../api";

function when(iso: string | null): string {
  if (!iso) return "—";
  const at = new Date(iso);
  const minutes = Math.round((Date.now() - at.getTime()) / 60000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

function number(value: number): string {
  return value.toLocaleString("en-US");
}

/* ---------------------------------------------------------------- Inbox --- */

/// What waits for a person. An agent that asks a question blocks its task; a task
/// in review holds a pull request whose checks are green. Both are one click away
/// from the decision they need, which is the whole point of the screen.
export function Inbox({ issues, onOpen }: { issues: Issue[]; onOpen: (id: string) => void }) {
  const blocked = issues.filter((issue) => issue.status === "blocked");
  const review = issues.filter((issue) => issue.status === "in_review");
  const rows = [...blocked, ...review];

  return (
    <section className="screen inbox">
      <PageHeader
        title="Inbox"
        subtitle={`${blocked.length} waiting on an answer · ${review.length} waiting on a merge`}
      />

      {rows.length === 0 ? (
        <div className="empty-inbox">
          <Icon name="inbox" size={28} />
          <strong>Inbox zero</strong>
          <span className="dim">Agents and reviewers show up here when they need a person.</span>
        </div>
      ) : (
        <ul className="inbox-list">
          {rows.map((issue) => (
            <li key={issue.id}>
              <button className="inbox-row" onClick={() => onOpen(issue.id)}>
                <span className="mono key">{issue.key}</span>
                <span className="title">{issue.title}</span>
                <span className={`chip ${issue.status}`}>
                  {issue.status === "blocked" ? "needs an answer" : "waiting for you"}
                </span>
                <Icon name="chevron" size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/* ------------------------------------------------------------- Projects --- */

const NO_PROJECT = "none";

export function Projects({
  projects,
  issues,
  milestones,
  workspace,
  onChanged,
  onOpenTasks,
}: {
  projects: Project[];
  issues: Issue[];
  milestones: Milestone[];
  workspace: Workspace;
  onChanged: () => Promise<void>;
  /** Open Tasks filtered to a project id, or to "none" for tasks without one. */
  onOpenTasks: (filter: string) => void;
}) {
  const [managing, setManaging] = useState<string | null>(null);
  const [toast, notify] = useToast();
  const unassigned = projectStats(null, issues);
  const unassignedTasks = issues.some((issue) => issue.projectId === null);
  const openMilestones = openMilestoneCount(milestones);
  return (
    <section className="screen projects">
      <PageHeader
        title="Projects"
        actions={
          <button className="primary" onClick={() => setManaging("new")}>
            <Icon name="plus" size={13} /> New project
          </button>
        }
      />

      <div className="pgrid">
        {projects.map((project) => (
          <ProjectCard
            key={project.id}
            project={project}
            issues={issues}
            onOpen={() => onOpenTasks(project.id)}
            onSettings={() => setManaging(project.id)}
          />
        ))}
        {unassignedTasks || openMilestones > 0 ? (
          <div className="pcard-wrap">
            <button
              className="pcard"
              aria-label="Open tasks with no project"
              onClick={() => onOpenTasks(NO_PROJECT)}
            >
              <span className="pc">
                <span className="pdot muted" aria-hidden="true" />
                <h3>No project</h3>
              </span>
              <span className="dim small">Tasks and milestones that don't belong to a project.</span>
              <span className="pstats two">
                <Stat value={unassigned.open} label="Open tasks" />
                <Stat value={openMilestones} label="Open milestones" />
              </span>
            </button>
          </div>
        ) : null}
      </div>
      {projects.length === 0 && !unassignedTasks && openMilestones === 0 ? (
        <div className="empty">
          <strong>No projects yet</strong>
          <span>A project groups tasks, repositories and milestones. Create one to get started.</span>
        </div>
      ) : null}
      {toast}
      {managing && (
        <ProjectDialog
          key={managing}
          workspace={workspace}
          project={projects.find((entry) => entry.id === managing) ?? null}
          projects={projects}
          issues={issues}
          reload={onChanged}
          notify={notify}
          onClose={() => setManaging(null)}
        />
      )}
    </section>
  );
}

function Stat({ value, label }: { value: number; label: string }) {
  return (
    <span className="pstat">
      <b>{value}</b>
      <small>{label}</small>
    </span>
  );
}

function ProjectCard({
  project,
  issues,
  onOpen,
  onSettings,
}: {
  project: Project;
  issues: Issue[];
  onOpen: () => void;
  onSettings: () => void;
}) {
  const stats = projectStats(project.id, issues);
  const shown = project.repos.slice(0, 2);
  const more = project.repos.length - shown.length;
  return (
    <div className="pcard-wrap">
      <button className="pcard" aria-label={`Open ${project.name} tasks`} onClick={onOpen}>
        <span className="pc">
          <span className="pdot" style={{ background: project.color }} aria-hidden="true" />
          <h3 className="truncate">{project.name}</h3>
          <span className="chip mono">{project.prefix ?? "—"}</span>
        </span>
        <span className="prepos">
          {shown.map((repo) => (
            <span key={repo.id} className="chip repo-chip">
              <Icon name="branch" size={11} />
              <span className="mono truncate">{repo.name}</span>
            </span>
          ))}
          {more > 0 ? <span className="chip mono">+{more}</span> : null}
          {project.repos.length === 0 ? <span className="dim small">No repositories yet</span> : null}
        </span>
        <span className="pstats">
          <Stat value={stats.open} label="Open tasks" />
          <Stat value={stats.openPrs} label="Open PRs" />
          <Stat value={stats.running} label="Agents running" />
        </span>
        <span className="prog-row">
          <span>Progress</span>
          <span>{stats.progress}%</span>
        </span>
        <span
          className="prog"
          role="progressbar"
          aria-label={`${project.name} progress`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={stats.progress}
        >
          <i style={{ width: `${stats.progress}%` }} />
        </span>
      </button>
      <button className="icon-btn pgear" aria-label={`Settings for ${project.name}`} onClick={onSettings}>
        <Icon name="gear" size={14} />
      </button>
    </div>
  );
}

/* ------------------------------------------------------------- Activity --- */

export function Activity({
  runners,
  issues,
  agents,
  onOpen,
}: {
  runners: RecentRun[];
  issues: Issue[];
  agents: Agent[];
  onOpen: (id: string) => void;
}) {
  const [loading, setLoading] = useState(true);
  if (loading) setLoading(false);

  return (
    <section className="screen activity">
      <PageHeader
        title="Activity"
        subtitle={`${runners.length} runs`}
      />

      <ul className="activity-list">
        {runners.map((run) => {
          const issue = issues.find((candidate) => candidate.id === run.issueId);
          const agent = agents.find((candidate) => candidate.id === run.agentId);
          return (
            <li key={run.id}>
              <button className="activity-row" onClick={() => issue && onOpen(issue.id)}>
                <span className={`dot ${run.status}`} aria-hidden="true" />
                <span className="mono key">{issue?.key ?? "—"}</span>
                <span className="who">{agent?.name ?? run.adapter}</span>
                <span className="dim mono">
                  {number(run.tokensIn)}/{number(run.tokensOut)}
                </span>
                <span className="dim">{when(run.startedAt)}</span>
                <span className={`chip ${run.status}`}>{run.status}</span>
              </button>
            </li>
          );
        })}
      </ul>
      {runners.length === 0 ? <p className="dim">No runs yet.</p> : null}
    </section>
  );
}

/** One loader for the two screens that read runs, so fine-grained polling stays in App. */
export async function recentRuns(workspaceId: string): Promise<RecentRun[]> {
  return loadRecentRuns(workspaceId, 150);
}

export type { Artifact, Milestone };
