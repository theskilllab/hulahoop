-- no-transaction
--
-- A task can belong to no project, and a task with no project has no
-- repository: its runs work in an empty scratch directory. So `repo_id` stops
-- being required on issues, runs and runner jobs. SQLite only allows that by
-- rebuilding each table.
--
-- Every column, index and foreign key is carried over unchanged; only the
-- `not null` on `repo_id` goes. Ids are preserved, so every child row stays
-- attached. Rows that already point at a repository keep it.
--
-- `one_active_run_per_repo` stays: a unique index treats NULLs as distinct, so
-- repo-less runs never serialize against each other while repository runs
-- still do.

PRAGMA foreign_keys = OFF;

create table issues_v3 (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    project_id text references projects(id) on delete cascade,
    repo_id text references repos(id) on delete cascade,
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
    run_count integer not null default 0,
    current_run_id text,
    merge_revision text,
    merged_at text,
    abandon_reason text,
    created_at text not null,
    updated_at text not null,
    pr_url text,
    pr_number integer,
    pr_branch text,
    turn_queue text not null default '[]',
    turn_served text not null default '[]',
    turns_used integer not null default 0,
    due_date text,
    assignee_type text,
    assignee_id text
);

insert into issues_v3 (
    id, workspace_id, project_id, repo_id, milestone_id, number, issue_key, title, body,
    status, priority, status_before_blocked, pinned_comment_id, run_cap_cleared_at,
    attempt, run_count, current_run_id, merge_revision, merged_at, abandon_reason,
    created_at, updated_at, pr_url, pr_number, pr_branch, turn_queue, turn_served,
    turns_used, due_date, assignee_type, assignee_id
)
select
    id, workspace_id, project_id, repo_id, milestone_id, number, issue_key, title, body,
    status, priority, status_before_blocked, pinned_comment_id, run_cap_cleared_at,
    attempt, run_count, current_run_id, merge_revision, merged_at, abandon_reason,
    created_at, updated_at, pr_url, pr_number, pr_branch, turn_queue, turn_served,
    turns_used, due_date, assignee_type, assignee_id
from issues;

drop table issues;
alter table issues_v3 rename to issues;

create unique index if not exists issues_by_key on issues(issue_key);
create index if not exists issues_by_milestone on issues(milestone_id);
create index if not exists issues_by_merged_at on issues(workspace_id, unixepoch(merged_at));

create table runs_v2 (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    repo_id text references repos(id) on delete cascade,
    agent_id text not null references agents(id),
    attempt integer not null default 1,
    base_bookmark text not null,
    base_revision text,
    head_revision text,
    head_bookmark text,
    candidate_bookmark text,
    changed_paths text not null default '[]',
    parent_run_id text references runs(id),
    runner_id text,
    claimed_at text,
    last_heartbeat_at text,
    pid integer,
    pgid integer,
    toolchain_lock text,
    prompt_revision text not null,
    sandbox text not null default 'v0-env',
    env_keys text not null default '[]',
    artifact_dir text not null,
    diff_sha text,
    flags text not null default '[]',
    status text not null default 'queued',
    started_at text,
    finished_at text,
    exit_code integer,
    error text,
    created_at text not null,
    commit_name text,
    commit_email text,
    pr_url text,
    pr_number integer,
    pr_branch text,
    tokens_in integer not null default 0,
    tokens_out integer not null default 0,
    model text not null default '',
    summary text,
    adapter_snapshot text,
    connector_id text,
    billing_source text,
    actual_model text,
    cache_read_tokens integer,
    cache_write_tokens integer,
    model_turns integer,
    tool_calls integer,
    api_equivalent_usd real,
    actual_cost_usd real,
    cost_source text,
    failure_category text,
    price_in_per_million real,
    price_out_per_million real
);

insert into runs_v2 (
    id, issue_id, repo_id, agent_id, attempt, base_bookmark, base_revision, head_revision,
    head_bookmark, candidate_bookmark, changed_paths, parent_run_id, runner_id, claimed_at,
    last_heartbeat_at, pid, pgid, toolchain_lock, prompt_revision, sandbox, env_keys,
    artifact_dir, diff_sha, flags, status, started_at, finished_at, exit_code, error,
    created_at, commit_name, commit_email, pr_url, pr_number, pr_branch, tokens_in,
    tokens_out, model, summary, adapter_snapshot, connector_id, billing_source,
    actual_model, cache_read_tokens, cache_write_tokens, model_turns, tool_calls,
    api_equivalent_usd, actual_cost_usd, cost_source, failure_category,
    price_in_per_million, price_out_per_million
)
select
    id, issue_id, repo_id, agent_id, attempt, base_bookmark, base_revision, head_revision,
    head_bookmark, candidate_bookmark, changed_paths, parent_run_id, runner_id, claimed_at,
    last_heartbeat_at, pid, pgid, toolchain_lock, prompt_revision, sandbox, env_keys,
    artifact_dir, diff_sha, flags, status, started_at, finished_at, exit_code, error,
    created_at, commit_name, commit_email, pr_url, pr_number, pr_branch, tokens_in,
    tokens_out, model, summary, adapter_snapshot, connector_id, billing_source,
    actual_model, cache_read_tokens, cache_write_tokens, model_turns, tool_calls,
    api_equivalent_usd, actual_cost_usd, cost_source, failure_category,
    price_in_per_million, price_out_per_million
from runs;

drop table runs;
alter table runs_v2 rename to runs;

create unique index if not exists one_active_run
    on runs(issue_id) where status in ('queued', 'running');
create unique index if not exists one_active_run_per_repo
    on runs(repo_id) where status in ('queued', 'running');
create index if not exists runs_by_issue on runs(issue_id, created_at);
create index if not exists runs_by_finished_at on runs(unixepoch(finished_at), issue_id);

create table runner_jobs_v2 (
    id text primary key,
    repo_id text references repos(id) on delete cascade,
    issue_id text references issues(id) on delete cascade,
    kind text not null,
    payload text not null default '{}',
    status text not null default 'queued',
    result text,
    error text,
    claimed_at text,
    finished_at text,
    created_at text not null
);

insert into runner_jobs_v2 (
    id, repo_id, issue_id, kind, payload, status, result, error, claimed_at, finished_at,
    created_at
)
select
    id, repo_id, issue_id, kind, payload, status, result, error, claimed_at, finished_at,
    created_at
from runner_jobs;

drop table runner_jobs;
alter table runner_jobs_v2 rename to runner_jobs;

create index if not exists runner_jobs_by_repo on runner_jobs(repo_id, status);
create index if not exists runner_jobs_lumi_lease on runner_jobs(kind, status, claimed_at);

PRAGMA foreign_keys = ON;
