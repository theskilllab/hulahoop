import type { Issue, IssueStatus } from "../api";

const COLUMNS: { status: IssueStatus; label: string }[] = [
  { status: "backlog", label: "Backlog" },
  { status: "ready", label: "Tests to write" },
  { status: "tests_in_progress", label: "Authoring" },
  { status: "tests_ready", label: "Tests frozen" },
  { status: "in_progress", label: "Implementing" },
  { status: "in_review", label: "Review" },
  { status: "merging", label: "Merging" },
  { status: "blocked", label: "Blocked" },
  { status: "done", label: "Done" },
];

export function StatusPill({ status }: { status: string }) {
  return <span className={`pill ${status}`}>{status.replaceAll("_", " ")}</span>;
}

export default function Board({
  issues,
  onOpen,
  onNew,
  hasRepo,
}: {
  issues: Issue[];
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
      <header className="board-head">
        <h1>Loop</h1>
        <button className="primary" onClick={onNew}>
          New issue
        </button>
      </header>
      <div className="columns">
        {COLUMNS.map((column) => {
          const items = issues.filter((issue) => issue.status === column.status);
          return (
            <div className="column" key={column.status}>
              <h2>
                {column.label} <span>{items.length}</span>
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
