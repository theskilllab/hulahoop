-- Six statuses, and the tables the workflow needs.
--
-- The board keeps Backlog, To do, In progress, Blocked, In review and Done. The
-- two test-authoring states and `merging` disappear: work is picked up from To do
-- and closed when its pull request merges, and the proof of a change is that
-- merge, not a replay of a frozen suite.

update issues set status = 'todo' where status in ('ready', 'tests_ready');
update issues set status = 'in_progress' where status in ('tests_in_progress');
update issues set status = 'in_review' where status = 'merging';
update issues set status_before_blocked = 'in_progress'
 where status_before_blocked in ('ready', 'tests_ready', 'tests_in_progress', 'merging');

-- A workspace owns one workflow document: the statuses, who works them, and where
-- each one leads. The board column order is derived from it.
create table workflows (
  workspace_id text primary key references workspaces(id) on delete cascade,
  source text not null,
  updated_at text not null
);

-- Skills are a shared library. An agent's AGENTS.md says how it behaves; a skill
-- is a procedure it loads when the task calls for it.
create table skills (
  id text primary key,
  workspace_id text not null references workspaces(id) on delete cascade,
  name text not null,
  description text not null default '',
  source text not null,
  created_at text not null,
  updated_at text not null
);
create index skills_by_workspace on skills(workspace_id, name);

-- What the agent is told, per agent, instead of one prompt per role.
alter table agents add column agents_md text not null default '';

create table agent_skills (
  agent_id text not null references agents(id) on delete cascade,
  skill_id text not null references skills(id) on delete cascade,
  primary key (agent_id, skill_id)
);
