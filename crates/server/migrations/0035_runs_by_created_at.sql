-- Memory staleness counts the runs created since a note was last used.
create index if not exists runs_by_created_at on runs(created_at);
