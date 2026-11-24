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
import ProjectManager from "./ProjectManager";
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

export function Projects({
  projects,
  issues,
  workspace,
  onChanged,
  onOpen,
}: {
  projects: Project[];
  issues: Issue[];
  workspace: Workspace;
  onChanged: () => Promise<void>;
  onOpen: (id: string) => void;
}) {
  const [managing, setManaging] = useState<string | null>(null);
  return (
    <section className="screen projects">
      <PageHeader
        title="Projects"
        subtitle={`${projects.length} projects`}
        actions={
          <button className="ghost" onClick={() => setManaging("new")}>
            New project
          </button>
        }
      />

      <div className="project-grid">
        {projects.map((project) => {
          const own = issues.filter((issue) => issue.projectId === project.id);
          const open = own.filter((issue) => !["done", "cancelled"].includes(issue.status)).length;
          return (
            <article key={project.id} className="project-card">
              <header>
                <span className="prefix">{project.prefix ?? "—"}</span>
                <strong>{project.name}</strong>
              </header>
              <dl>
                <div>
                  <dt>open</dt>
                  <dd>{open}</dd>
                </div>
                <div>
                  <dt>done</dt>
                  <dd>{own.length - open}</dd>
                </div>
                <div>
                  <dt>repositories</dt>
                  <dd>{project.repos.length}</dd>
                </div>
              </dl>
              <ul className="repo-list">
                {project.repos.map((repo) => (
                  <li key={repo.id}>
                    <Icon name="branch" size={13} />
                    <span className="mono">{repo.name}</span>
                    <span className="dim">{repo.remoteUrl ?? "no url"}</span>
                  </li>
                ))}
                {project.repos.length === 0 ? <li className="dim">No repository attached yet.</li> : null}
              </ul>
              <footer>
                <button
                  className="link"
                  onClick={() => {
                    const first = own[0];
                    if (first) onOpen(first.id);
                    else setManaging(project.id);
                  }}
                >
                  {own[0] ? `Open ${own[0].key}` : "Attach a repository"}
                </button>
                <button className="link" onClick={() => setManaging(project.id)}>Project settings</button>
              </footer>
            </article>
          );
        })}
        {projects.length === 0 ? <p className="dim">No projects yet. Create one here.</p> : null}
      </div>
      {managing && (
        <ProjectManager
          key={managing}
          workspace={workspace}
          project={projects.find((entry) => entry.id === managing) ?? null}
          onChanged={onChanged}
          onClose={() => setManaging(null)}
        />
      )}
    </section>
  );
}

/* --------------------------------------------------------- Pull requests --- */

export function PullRequests({ issues, onOpen }: { issues: Issue[]; onOpen: (id: string) => void }) {
  const withPr = issues.filter((issue) => issue.prUrl);
  return (
    <section className="screen prs">
      <PageHeader
        title="Pull requests"
        subtitle={`${withPr.length} from GitHub`}
      />

      {withPr.length === 0 ? (
        <p className="dim">No pull requests yet.</p>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>task</th>
              <th>title</th>
              <th>pull</th>
              <th>status</th>
            </tr>
          </thead>
          <tbody>
            {withPr.map((issue) => (
              <tr key={issue.id} onClick={() => onOpen(issue.id)} className="clickable">
                <td className="mono">{issue.key}</td>
                <td>{issue.title}</td>
                <td className="mono">
                  <a href={issue.prUrl ?? "#"} target="_blank" rel="noreferrer">
                    #{issue.prNumber}
                  </a>
                </td>
                <td>
                  <span className={`pill ${issue.status}`}>{issue.status.replaceAll("_", " ")}</span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
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
