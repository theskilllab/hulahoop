-- Tasks revamp: due dates, an informational assignee, files and run-linked
-- comments, and the end of acceptance criteria and frozen test suites.

-- A due date is a plain calendar day (YYYY-MM-DD); nothing schedules on it.
alter table issues add column due_date text;

-- Who the task is "with", for people scanning the board. It never decides
-- dispatch: WORKFLOWS.md does. assignee_type is 'human' or 'agent'. For an
-- agent, assignee_id is the agent's uuid string. The local human is the only
-- human, so a human assignee keeps assignee_id null.
alter table issues add column assignee_type text;
alter table issues add column assignee_id text;

-- Priorities are a fixed ladder now; anything else becomes medium.
update issues set priority = 'medium'
where priority is null or priority not in ('urgent', 'high', 'medium', 'low');

-- Files attached to a task are ordinary artifacts that point at it.
alter table artifacts add column issue_id blob references issues(id) on delete set null;
create index artifacts_by_issue on artifacts(issue_id) where issue_id is not null;

-- Comments an agent posts during a run link back to that run.
alter table comments add column run_id blob references runs(id) on delete set null;
-- People named with @ in a comment, as a JSON array of {type, id, name}.
alter table comments add column mentions text not null default '[]';

-- Acceptance criteria fold into the task body as a checklist so nothing a
-- person wrote is lost, then the table and the frozen-suite columns go.
update issues set body = body || char(10) || char(10) || '## Acceptance criteria' || char(10) || char(10) || (
    select group_concat('- [ ] ' || c.text, char(10) order by c.position)
    from criteria c where c.issue_id = issues.id
)
where exists (select 1 from criteria c where c.issue_id = issues.id);

drop table criteria;

alter table issues drop column tests_bookmark;
alter table issues drop column tests_revision;
alter table issues drop column tests_frozen_at;
alter table issues drop column suite_seq;
alter table issues drop column frozen_paths;
