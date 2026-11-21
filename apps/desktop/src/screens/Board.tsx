import type { Issue, IssueStatus } from "../api";
import PageHeader from "../PageHeader";

/// The board is a view of the workspace's workflow document, so the columns and
/// their wording arrive as props. This is the fallback for a workspace that has
/// not loaded one yet.
const FALLBACK: Column[] = [
  { status: "backlog", title: "Backlog", owner: "human" },
  { status: "todo", title: "To do", owner: "human" },
  { status: "in_progress", title: "In progress", owner: "agent" },
  { status: "blocked", title: "Blocked", owner: "human" },
  { status: "in_review", title: "In review", owner: "human" },
  { status: "done", title: "Done", owner: "human" },
];

export type Column = { status: IssueStatus; title: string; owner: string };

export function StatusPill({ status }: { status: string }) {
  return <span className={`pill ${status}`}>{status.replaceAll("_", " ")}</span>;
}

export default function Board({
  issues,
  columns = FALLBACK,
  onOpen,
  onNew,
  hasRepo,
}: {
  issues: Issue[];
  columns?: Column[];
  onOpen: (id: string) => void;
  onNew: () => void;
  hasRepo: boolean;
}) {
  if (!hasRepo) {
    return (
      <section className="empty">
        <h2>No repository yet</h2>
        <p>
          Hulahoop needs a repository that declares its commands: a <code>flake.nix</code> and a <code>justfile</code> with
          <code> setup</code> and <code>test</code> recipes. Add one from Settings, then create your first issue.
        </p>
      </section>
    );
  }

  return (
    <section className="board">
      <PageHeader
        title="Tasks"
        subtitle={`${issues.length} open · agents work the middle columns, you decide the ones that ask`}
        actions={
          <button className="primary" onClick={onNew}>
            New issue
          </button>
        }
      />
      <div className="columns">
        {columns.map((column) => {
          const items = issues.filter((issue) => issue.status === column.status);
          return (
            <div className="column" key={column.status}>
              <h2 title={column.owner === "agent" ? "an agent works this column" : "a person works this column"}>
                {column.title} <span>{items.length}</span>
                {column.owner === "agent" ? <em className="owner">agent</em> : null}
              </h2>
              {items.map((issue) => (
                <button className="card" key={issue.id} onClick={() => onOpen(issue.id)}>
                  <span className="title">
                    <code className="key">{issue.key}</code> {issue.title}
                  </span>
                  <span className="meta">
                    <StatusPill status={issue.status} />
                    <span className="dim">
                      {issue.criteria.length} criteria · attempt {issue.attempt || 0}
                    </span>
                  </span>
                  {issue.runs.length > 0 && (
                    <span className="timeline">
                      {issue.runs.slice(-4).map((run) => (
                        <span key={run.id} className={`dot ${run.status}`} title={`${run.role} ${run.status}`} />
                      ))}
                    </span>
                  )}
                </button>
              ))}
              {items.length === 0 && <p className="dim small">empty</p>}
            </div>
          );
        })}
      </div>
    </section>
  );
}
