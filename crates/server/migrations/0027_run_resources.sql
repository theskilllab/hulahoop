-- What a run used of the host, measured by the runner while it ran: the peak
-- memory of its process group, the cores it kept busy on average, and how much
-- its checkout grew. The server predicts the next run's need from these, so a
-- run starts only when the host has room for it. Older runs stay unknown.
alter table runs add column peak_memory_bytes integer;
alter table runs add column cpu_cores real;
alter table runs add column disk_growth_bytes integer;

-- Why a queued run has not started yet, in words a person reads on the board:
-- `Waiting for memory: needs ~2.4 GB, 1.1 GB free`. Null once it is claimed.
alter table runs add column wait_reason text;

-- Runs on one repository share the host instead of taking turns at it: each
-- task has its own checkout and the runner serializes the shared mirror, so
-- the one-live-run-per-repository index goes. One live run per task stays.
drop index if exists one_active_run_per_repo;
