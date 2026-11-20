-- A run reports what it spent. The transcript is what the run already wrote,
-- so these are totals over its finished messages rather than a second source of
-- truth. Existing rows spent an unknown amount, which reads as zero.
alter table runs add column tokens_in integer not null default 0;
alter table runs add column tokens_out integer not null default 0;
