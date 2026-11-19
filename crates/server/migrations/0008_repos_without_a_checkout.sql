-- no-transaction
--
-- A repository can now be attached from the GitHub app's listing, without a
-- checkout on this machine: the runner clones it in local mode. So `path` stops
-- being required. SQLite only allows that by rebuilding the table.
PRAGMA foreign_keys = OFF;

create table repos_v2 (
    id text primary key not null,
    project_id text not null references projects(id) on delete cascade,
    name text not null,
    path text,
    remote_url text,
    vcs text not null default 'jj',
    base_revision text,
    base_bookmark text not null default 'main',
    contract text not null default 'nix+just',
    contract_version text not null,
    fetched_at text,
    created_at text not null
);

insert into repos_v2 (id, project_id, name, path, remote_url, vcs, base_revision, base_bookmark, contract, contract_version, fetched_at, created_at)
select id, project_id, name, path, remote_url, vcs, base_revision, base_bookmark, contract, contract_version, fetched_at, created_at
from repos;

drop table repos;
alter table repos_v2 rename to repos;

PRAGMA foreign_keys = ON;
