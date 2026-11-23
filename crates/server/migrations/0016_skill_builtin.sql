-- A built-in skill is one the workspace gets without anybody writing it: the run
-- contract, which carries the markers the server parses. It is shown, never
-- edited, and attached to every agent.
alter table skills add column builtin integer not null default 0;

-- Runs recorded the old pipeline's stage names. What a turn did is now read from
-- what it produced, so every past stage was work.
update runs set role = 'worker' where role in ('test_author', 'implementer');
