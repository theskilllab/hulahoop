-- Long-term memory: short notes agents and people keep across turns and tasks.
-- `scope_ref` says whose note it is: the project id for `project`, the agent or
-- person name for `agent` and `person`, empty for `workspace`.
create table memories (
    id text primary key,
    workspace_id text not null references workspaces(id) on delete cascade,
    scope text not null check (scope in ('workspace', 'project', 'person', 'agent')),
    scope_ref text not null default '',
    kind text not null check (kind in ('decision', 'convention', 'preference', 'fact', 'gotcha')),
    title text not null,
    body text not null,
    source_issue_id text references issues(id) on delete set null,
    source_run_id text references runs(id) on delete set null,
    source_comment_id text references comments(id) on delete set null,
    author text not null default '',
    created_at text not null,
    updated_at text not null,
    last_used_at text,
    use_count integer not null default 0,
    replaced_by text references memories(id) on delete set null,
    archived_at text
);
create index memories_scope on memories (workspace_id, scope, scope_ref);

-- Keyword search over title and body, kept in step by triggers.
create virtual table memories_fts using fts5(
    title, body, content = 'memories', content_rowid = 'rowid'
);
create trigger memories_fts_insert after insert on memories begin
    insert into memories_fts (rowid, title, body) values (new.rowid, new.title, new.body);
end;
create trigger memories_fts_delete after delete on memories begin
    insert into memories_fts (memories_fts, rowid, title, body)
        values ('delete', old.rowid, old.title, old.body);
end;
create trigger memories_fts_update after update of title, body on memories begin
    insert into memories_fts (memories_fts, rowid, title, body)
        values ('delete', old.rowid, old.title, old.body);
    insert into memories_fts (rowid, title, body) values (new.rowid, new.title, new.body);
end;
