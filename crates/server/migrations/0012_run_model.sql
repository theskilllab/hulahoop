-- A run remembers the model it was given. The agent's model can change later, and
-- a cost report that re-reads the roster would quietly rewrite history.
alter table runs add column model text not null default '';

update runs
set model = coalesce((select agents.model from agents where agents.id = runs.agent_id), '')
where model = '';

-- Codex is no longer a supported harness: pi covers those providers, so the
-- connector has nobody left to serve.
delete from connectors where provider = 'codex';
