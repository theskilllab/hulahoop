-- no-transaction
--
-- Three things arrive together because they share one idea: work has a name, work
-- has a shelf, and the shelf can hold files.
--
--   * a project carries a prefix (BAL-15, ATL-55), and an issue may belong to no
--     project at all — so `issues.project_id` stops being required, which SQLite
--     only allows by rebuilding the table;
--   * milestones are named bodies of work an issue can be pointed at;
--   * artifacts are files, optionally filed under a milestone.
--
-- The rebuild turns foreign keys off, which cannot be done inside a transaction,
-- hence the directive above. Ids are preserved, so every child row stays attached.

PRAGMA foreign_keys = OFF;

create table if not exists milestones (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    project_id text references projects(id) on delete cascade,
    title text not null,
    body text not null default '',
    status text not null default 'open',
    target_date text,
    created_at text not null,
    updated_at text not null
);

create table if not exists artifacts (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    project_id text references projects(id) on delete set null,
    milestone_id text references milestones(id) on delete set null,
    title text not null,
    kind text not null default 'document',
    filename text not null,
    mime text not null default 'application/octet-stream',
    size integer not null default 0,
    sha256 text not null default '',
    created_at text not null
);

create index if not exists artifacts_by_milestone on artifacts(milestone_id);

alter table workspaces add column default_prefix text not null default 'WS';
alter table projects add column prefix text;

create table issues_v2 (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    project_id text references projects(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
    milestone_id text references milestones(id) on delete set null,
    number integer not null default 0,
    issue_key text not null default '',
    title text not null,
    body text not null default '',
    status text not null default 'backlog',
    priority text not null default 'medium',
    status_before_blocked text,
    pinned_comment_id text,
    run_cap_cleared_at text,
    attempt integer not null default 0,
    suite_seq integer not null default 0,
    run_count integer not null default 0,
    current_run_id text,
    tests_revision text,
    tests_bookmark text,
    frozen_paths text not null default '[]',
    tests_frozen_at text,
    merge_revision text,
    merged_at text,
    abandon_reason text,
    created_at text not null,
    updated_at text not null
);

insert into issues_v2 (
    id, workspace_id, project_id, repo_id, milestone_id, number, issue_key, title, body,
    status, priority, status_before_blocked, pinned_comment_id, run_cap_cleared_at,
    attempt, suite_seq, run_count, current_run_id, tests_revision, tests_bookmark,
    frozen_paths, tests_frozen_at, merge_revision, merged_at, abandon_reason,
    created_at, updated_at
)
select
    i.id, p.workspace_id, i.project_id, i.repo_id, null,
    row_number() over (partition by i.project_id order by i.created_at),
    coalesce(upper(substr(p.name, 1, 3)), 'WS') || '-' || row_number() over (partition by i.project_id order by i.created_at),
    i.title, i.body, i.status, i.priority, i.status_before_blocked, i.pinned_comment_id,
    i.run_cap_cleared_at, i.attempt, i.suite_seq, i.run_count, i.current_run_id,
    i.tests_revision, i.tests_bookmark, i.frozen_paths, i.tests_frozen_at,
    i.merge_revision, i.merged_at, i.abandon_reason, i.created_at, i.updated_at
from issues i join projects p on p.id = i.project_id;

drop table issues;
alter table issues_v2 rename to issues;

create unique index if not exists issues_by_key on issues(issue_key);
create index if not exists issues_by_milestone on issues(milestone_id);

PRAGMA foreign_keys = ON;
