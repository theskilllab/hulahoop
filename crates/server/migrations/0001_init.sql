-- Hulahoop schema, contract 26.11.
-- One SQLite writer: the server. The runner reports over HTTP and never opens
-- this file, which is what makes the partial unique indexes sufficient.

create table if not exists users (
    id text primary key,
    name text not null,
    email text,
    created_at text not null
);

create table if not exists workspaces (
    id text primary key,
    name text not null,
    slug text not null unique,
    created_at text not null
);

create table if not exists workspace_members (
    workspace_id text not null references workspaces(id) on delete cascade,
    user_id text not null references users(id) on delete cascade,
    role text not null,
    primary key (workspace_id, user_id)
);

create table if not exists projects (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    name text not null,
    created_at text not null
);

create table if not exists repos (
    id text primary key,
    project_id text not null references projects(id) on delete cascade,
    name text not null,
    path text not null,
    vcs text not null default 'jj',
    base_revision text,
    base_bookmark text not null default 'main',
    contract text not null default 'nix+just',
    contract_version text not null,
    fetched_at text,
    created_at text not null
);

create table if not exists agents (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    name text not null,
    adapter text not null,
    model text not null,
    created_at text not null,
    updated_at text not null
);

create table if not exists agent_preflight (
    agent_id text not null references agents(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
    ok integer not null,
    detail text not null,
    checked_at text not null,
    primary key (agent_id, repo_id)
);

create table if not exists project_agents (
    project_id text not null references projects(id) on delete cascade,
    agent_id text not null references agents(id) on delete cascade,
    role text not null,
    primary key (project_id, role)
);

create table if not exists issues (
    id text primary key,
    project_id text not null references projects(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
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

create index if not exists issues_by_project on issues(project_id, status);

create table if not exists criteria (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    position integer not null,
    text text not null,
    test_name text,
    unique (issue_id, position)
);

create table if not exists checkouts (
    id text primary key,
    project_id text not null references projects(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
    owner_kind text not null,
    owner_id text not null,
    provider text not null,
    name text not null,
    path text not null,
    state text not null default 'creating',
    created_at text not null,
    cleaned_at text,
    unique (owner_kind, owner_id)
);

create table if not exists runs (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
    agent_id text not null references agents(id),
    role text not null,
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
    created_at text not null
);

create unique index if not exists one_active_run
    on runs(issue_id) where status in ('queued', 'running');

create unique index if not exists one_active_run_per_repo
    on runs(repo_id) where status in ('queued', 'running');

create index if not exists runs_by_issue on runs(issue_id, created_at);

create table if not exists replays (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    repo_id text not null references repos(id) on delete cascade,
    run_id text references runs(id) on delete set null,
    candidate_run_id text references runs(id) on delete set null,
    approval_id text,
    kind text not null,
    attempt integer not null default 1,
    revision text,
    tests_bookmark text,
    status text not null default 'queued',
    exit_code integer,
    ancestry_ok integer,
    artifact_dir text not null,
    error text,
    created_at text not null,
    finished_at text
);

create unique index if not exists one_active_replay_per_issue
    on replays(issue_id) where status in ('queued', 'running');

create table if not exists reviews (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    reviewed_run_id text not null references runs(id) on delete cascade,
    reviewer_run_id text references runs(id) on delete set null,
    verdict text not null,
    body text not null default '',
    created_at text not null
);

create index if not exists reviews_by_run on reviews(reviewed_run_id);

create table if not exists comments (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    author_type text not null,
    author_id text,
    body text not null,
    pinned integer not null default 0,
    created_at text not null
);

create table if not exists approvals (
    id text primary key,
    issue_id text not null references issues(id) on delete cascade,
    run_id text not null references runs(id) on delete cascade,
    head_revision text not null,
    diff_sha text not null,
    landing_bookmark text,
    acknowledged_flags text not null default '[]',
    approved_by text not null,
    voided_at text,
    created_at text not null
);

create table if not exists repo_locks (
    repo_id text primary key references repos(id) on delete cascade,
    holder text not null,
    acquired_at text not null,
    expires_at text not null
);

create table if not exists run_events (
    id text primary key,
    run_id text not null references runs(id) on delete cascade,
    seq integer not null,
    kind text not null,
    payload text not null default '{}',
    created_at text not null,
    unique (run_id, seq)
);

create table if not exists meta (
    key text primary key,
    value text not null
);

-- Work the runner executes that is not a suite run: preflight probes, base
-- fetches, and merge verification. Replays stay reserved for `just test`.
create table if not exists runner_jobs (
    id text primary key,
    repo_id text not null references repos(id) on delete cascade,
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

create index if not exists runner_jobs_by_repo on runner_jobs(repo_id, status);
