import { useEffect, useState } from "react";

import {
  gql,
  loadModels,
  loadRecentRuns,
  type Agent,
  type Artifact,
  type Issue,
  type Milestone,
  type Project,
  type RecentRun,
} from "../api";
import Icon from "../Icon";

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
      <header>
        <div>
          <h1>Inbox</h1>
          <p className="dim">
            {rows.length === 0
              ? "Nothing waits for you. Agents work the board; you decide the ones that ask."
              : `${blocked.length} waiting on an answer, ${review.length} waiting on a merge.`}
          </p>
        </div>
      </header>

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
  agents,
  onOpenSettings,
  onOpen,
}: {
  projects: Project[];
  issues: Issue[];
  agents: Agent[];
  onOpenSettings: () => void;
  onOpen: (id: string) => void;
}) {
  return (
    <section className="screen projects">
      <header>
        <div>
          <h1>Projects</h1>
          <p className="dim">
            A project holds repositories, carries the prefix on every task, and binds the agents its
            statuses call.
          </p>
        </div>
        <button className="ghost" onClick={onOpenSettings}>
          Manage in settings
        </button>
      </header>

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
                    <span className="dim">{repo.path ?? "mirror"}</span>
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
                    else onOpenSettings();
                  }}
                >
                  {own[0] ? `Open ${own[0].key}` : "Attach a repository"}
                </button>
                <span className="dim">{agents.length} agents in this workspace</span>
              </footer>
            </article>
          );
        })}
        {projects.length === 0 ? <p className="dim">No projects yet. Create one in Settings.</p> : null}
      </div>
    </section>
  );
}

/* ----------------------------------------------------------- Connectors --- */

type Connection = { provider: string; kind: string; connected: boolean; label: string | null };

export function Connectors({ onOpenSettings }: { onOpenSettings: () => void }) {
  const [rows, setRows] = useState<Connection[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  if (!loaded) {
    setLoaded(true);
    void gql<{ connections: Connection[] }>(
      `query { connections { provider kind connected label } }`,
    )
      .then((data) => setRows(data.connections))
      .catch((problem) => setError(String(problem instanceof Error ? problem.message : problem)));
  }

  const known = ["github", "claude_code", "pi"];
  const shown = known.map((provider) => rows.find((row) => row.provider === provider) ?? {
    provider,
    kind: "—",
    connected: false,
    label: null,
  });

  return (
    <section className="screen connectors">
      <header>
        <div>
          <h1>Connectors</h1>
          <p className="dim">
            What the agents sign in with. A connector is sealed on the host and handed to a run of
            that harness only.
          </p>
        </div>
        <button className="primary" onClick={onOpenSettings}>
          Connect
        </button>
      </header>

      {error ? <p className="error">{error}</p> : null}

      <ul className="connector-list">
        {shown.map((row) => (
          <li key={row.provider}>
            <Icon name={row.provider === "github" ? "branch" : "bot"} size={16} />
            <div>
              <strong>{row.provider}</strong>
              <span className="dim">{row.kind === "—" ? "nothing stored" : row.kind.replaceAll("_", " ")}</span>
            </div>
            <span className={row.connected ? "ok" : "dim"}>
              {row.connected ? "connected" : "not connected"}
            </span>
            <span className="dim mono">{row.label ?? ""}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/* ----------------------------------------------------------------- Cost --- */

export function Cost({ runners, agents }: { runners: RecentRun[]; agents: Agent[] }) {
  const [prices, setPrices] = useState<Map<string, { input: number; output: number }>>(new Map());

  // Prices come from the harness catalogue, not from a table in the app: pi
  // publishes dollars per million tokens next to each model.
  useEffect(() => {
    const harnesses = [...new Set(agents.map((agent) => agent.adapter))];
    let live = true;
    Promise.all(harnesses.map((harness) => loadModels(harness).catch(() => [])))
      .then((lists) => {
        if (!live) return;
        const next = new Map<string, { input: number; output: number }>();
        for (const entry of lists.flat()) {
          if (entry.costIn !== null && entry.costOut !== null) {
            next.set(entry.id, { input: entry.costIn, output: entry.costOut });
          }
        }
        setPrices(next);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [agents]);

  const spend = (run: RecentRun) => {
    const price = prices.get(run.model);
    if (!price) return null;
    return (run.tokensIn * price.input + run.tokensOut * price.output) / 1_000_000;
  };

  const byAgent = new Map<string, { input: number; output: number; runs: number }>();
  for (const run of runners) {
    const entry = byAgent.get(run.agentId) ?? { input: 0, output: 0, runs: 0 };
    entry.input += run.tokensIn;
    entry.output += run.tokensOut;
    entry.runs += 1;
    byAgent.set(run.agentId, entry);
  }
  const totals = runners.reduce(
    (sum, run) => ({ input: sum.input + run.tokensIn, output: sum.output + run.tokensOut }),
    { input: 0, output: 0 },
  );
  const biggest = Math.max(1, ...[...byAgent.values()].map((entry) => entry.input + entry.output));
  const failed = runners.filter((run) => run.status === "failed").length;
  const priced = runners.map(spend).filter((value): value is number => value !== null);
  const spent = priced.reduce((sum, value) => sum + value, 0);

  return (
    <section className="screen cost">
      <header>
        <div>
          <h1>Cost control</h1>
          <p className="dim">
            Tokens the agents reported, from {runners.length} recent runs. Nothing is estimated: a run
            that reported nothing counts as nothing.
          </p>
        </div>
      </header>

      <div className="stat-row">
        <div className="stat">
          <span className="label">input</span>
          <strong>{number(totals.input)}</strong>
        </div>
        <div className="stat">
          <span className="label">output</span>
          <strong>{number(totals.output)}</strong>
        </div>
        <div className="stat">
          <span className="label">runs</span>
          <strong>{runners.length}</strong>
        </div>
        <div className="stat">
          <span className="label">failed</span>
          <strong>{failed}</strong>
        </div>
        <div className="stat">
          <span className="label">estimated spend</span>
          <strong>{priced.length ? `$${spent.toFixed(4)}` : "—"}</strong>
        </div>
      </div>

      <table className="table">
        <thead>
          <tr>
            <th>agent</th>
            <th>runs</th>
            <th>input</th>
            <th>output</th>
            <th>share</th>
          </tr>
        </thead>
        <tbody>
          {[...byAgent.entries()]
            .sort((a, b) => b[1].input + b[1].output - (a[1].input + a[1].output))
            .map(([agentId, entry]) => {
              const agent = agents.find((candidate) => candidate.id === agentId);
              const total = entry.input + entry.output;
              return (
                <tr key={agentId}>
                  <td>{agent?.name ?? agentId.slice(0, 8)}</td>
                  <td>{entry.runs}</td>
                  <td className="mono">{number(entry.input)}</td>
                  <td className="mono">{number(entry.output)}</td>
                  <td>
                    <span className="meter">
                      <span style={{ width: `${Math.round((total / biggest) * 100)}%` }} />
                    </span>
                  </td>
                </tr>
              );
            })}
        </tbody>
      </table>
      {byAgent.size === 0 ? <p className="dim">No runs yet, so nothing spent.</p> : null}
    </section>
  );
}

/* --------------------------------------------------------- Pull requests --- */

export function PullRequests({ issues, onOpen }: { issues: Issue[]; onOpen: (id: string) => void }) {
  const withPr = issues.filter((issue) => issue.prUrl);
  return (
    <section className="screen prs">
      <header>
        <div>
          <h1>Pull requests</h1>
          <p className="dim">
            Opened when the implementation is green, so a person reviews a pull request rather than a
            branch.
          </p>
        </div>
      </header>

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
      <header>
        <div>
          <h1>Activity</h1>
          <p className="dim">Every run, newest first. A run that says nothing is a run that changed nothing.</p>
        </div>
      </header>

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
                <span className="role">{run.role.replaceAll("_", " ")}</span>
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
