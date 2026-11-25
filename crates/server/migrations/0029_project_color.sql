-- A project has a colour, shown beside its name wherever projects are listed.
-- Existing projects take the palette in the order they were made, so each
-- workspace starts out with distinct colours.
alter table projects add column color text not null default '#2f7ad8';

update projects
set color = case (
    select count(*) from projects earlier
    where earlier.workspace_id = projects.workspace_id
      and (earlier.created_at < projects.created_at
           or (earlier.created_at = projects.created_at and earlier.id < projects.id))
) % 8
    when 0 then '#2f7ad8'
    when 1 then '#1f9d7a'
    when 2 then '#c2692b'
    when 3 then '#6a5acd'
    when 4 then '#d4558a'
    when 5 then '#b88100'
    when 6 then '#d03b3b'
    else '#64748b'
end;
