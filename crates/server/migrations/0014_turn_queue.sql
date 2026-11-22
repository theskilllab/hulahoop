-- A status may name several agents; they take turns in the order the workflow
-- lists them. The queue is stored per task so a turn survives a restart, and
-- `turns_used` is what the `Turns:` cap counts.
alter table issues add column turn_queue text not null default '[]';
alter table issues add column turn_served text not null default '[]';
alter table issues add column turns_used integer not null default 0;
