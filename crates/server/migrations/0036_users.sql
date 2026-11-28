-- Real logins. `users` keeps its rows (the local `you` becomes the first owner
-- when the setup link is used); a password hash and a unique email make them
-- people. Tokens are stored as sha256 hashes, never as they were handed out.

alter table users add column password_hash text;
create unique index if not exists users_by_email on users(email) where email is not null;

create table if not exists sessions (
    token_hash text primary key,
    user_id blob not null references users(id) on delete cascade,
    created_at text not null,
    expires_at integer not null
);

create table if not exists api_tokens (
    id blob primary key,
    user_id blob not null references users(id) on delete cascade,
    name text not null,
    token_hash text not null unique,
    created_at text not null,
    revoked_at text
);

create index if not exists sessions_by_user on sessions(user_id, expires_at);
create index if not exists sessions_by_expiry on sessions(expires_at);
create index if not exists api_tokens_by_user on api_tokens(user_id);
create index if not exists workspace_members_by_user on workspace_members(user_id);

create table if not exists invites (
    token_hash text primary key,
    role text not null,
    created_by blob references users(id) on delete set null,
    created_at text not null,
    expires_at integer not null,
    used_at text
);

-- Status moves with the person who made them. A move by the server itself has
-- no actor.
create table if not exists issue_events (
    id blob primary key,
    issue_id blob not null references issues(id) on delete cascade,
    kind text not null,
    from_status text,
    to_status text,
    actor_id blob references users(id) on delete set null,
    created_at text not null
);

create index if not exists issue_events_by_issue on issue_events(issue_id, created_at);
