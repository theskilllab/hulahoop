-- Pull requests, read from GitHub and reviewed from Hulahoop.
--
-- A person reviews and merges as themselves, so their own GitHub token is kept
-- per user and sealed with the vault like every other secret. What the token
-- showed when it was checked is kept beside it, so the settings screen can say
-- what is missing without asking GitHub again.
create table personal_tokens (
    user_id blob not null references users(id) on delete cascade,
    provider text not null,
    secret_ciphertext text not null,
    login text not null,
    name text,
    avatar_url text,
    token_hint text not null,
    problems text not null default '[]',
    validated_at text not null,
    primary key (user_id, provider)
);

-- Every row below belongs to a repository and goes with it: a removed
-- repository must not leave its pull requests in the list or the badge.
--
-- The list of pull requests is a cache the server keeps in step with GitHub, so
-- a screen that polls every few seconds never waits on GitHub. Review and CI
-- state are folded into one word each when the pull request is synced.
create table pull_requests (
    repo_id blob not null references repos(id) on delete cascade,
    number integer not null,
    repo_name text not null,
    title text not null,
    url text not null,
    state text not null,
    draft integer not null default 0,
    merged integer not null default 0,
    author_login text not null,
    author_avatar_url text,
    head_ref text not null,
    head_sha text not null,
    base_ref text not null,
    additions integer not null default 0,
    deletions integer not null default 0,
    changed_files integer not null default 0,
    review_state text not null,
    requested_reviewers text not null default '[]',
    ci_state text not null default 'NONE',
    ci_checked_at text,
    created_at text not null,
    updated_at text not null,
    merged_at text,
    closed_at text,
    synced_at text not null,
    primary key (repo_id, number)
);

create index pull_requests_by_update on pull_requests (updated_at);

-- Line comments a person wrote that GitHub has not seen yet. With a token they
-- go out with the next review; without one they stay here.
create table pull_request_comments (
    id blob primary key,
    repo_id blob not null references repos(id) on delete cascade,
    number integer not null,
    path text not null,
    line integer not null,
    side text not null,
    body text not null,
    author_login text not null,
    created_at text not null
);

create index pull_request_comments_by_pull on pull_request_comments (repo_id, number);

-- Per repository: the merge method used last, and what the last sync could not
-- reach, so one repository without access does not hide the others.
create table pull_request_repos (
    repo_id blob primary key references repos(id) on delete cascade,
    last_merge_method text,
    synced_at text,
    sync_problem text
);
