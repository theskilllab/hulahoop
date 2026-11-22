-- Two connectors can share a provider: Claude Code accepts a plan login *or* a
-- billed API key, and they are separate purchases with separate lifetimes. The
-- row identity is therefore (provider, kind), and the screen's own id
-- (`claude-sub`, `claude-api`) is derived from that pair.
create table connectors_v2 (
    id blob primary key,
    provider text not null,
    kind text not null,
    secret_ciphertext text not null,
    label text,
    created_at text not null,
    updated_at text not null,
    unique (provider, kind)
);

insert into connectors_v2 (id, provider, kind, secret_ciphertext, label, created_at, updated_at)
select id, provider, kind, secret_ciphertext, label, created_at, updated_at from connectors;

drop table connectors;
alter table connectors_v2 rename to connectors;

-- Which connector an agent authenticates with, when its harness offers a choice.
-- Null means "whichever one is connected", which is what one-entry harnesses do.
alter table agents add column connector text;
