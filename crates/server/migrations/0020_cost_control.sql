-- Run snapshots are nullable: old transcripts and absent provider usage have unknown cost.
alter table runs add column adapter_snapshot text;
alter table runs add column connector_id text;
alter table runs add column billing_source text;
alter table runs add column actual_model text;
alter table runs add column cache_read_tokens integer;
alter table runs add column cache_write_tokens integer;
alter table runs add column model_turns integer;
alter table runs add column tool_calls integer;
alter table runs add column api_equivalent_usd real;
alter table runs add column actual_cost_usd real;
alter table runs add column cost_source text;
alter table runs add column failure_category text;
alter table runs add column price_in_per_million real;
alter table runs add column price_out_per_million real;

-- Match the UTC epoch predicates in period reporting so SQLite can seek.
create index runs_by_finished_at on runs(unixepoch(finished_at), issue_id);
create index issues_by_merged_at on issues(workspace_id, unixepoch(merged_at));

create table cost_settings (
    workspace_id text primary key references workspaces(id) on delete cascade,
    api_budget_usd real,
    success_target_percent real,
    updated_at text not null
);

-- Plans are workspace-specific and advisory. Their fee is monthly cash
-- spend; their token allowance is a configured estimate.
create table cost_plans (
    workspace_id text not null references workspaces(id) on delete cascade,
    connector_id text not null,
    label text not null,
    monthly_fee_usd real not null,
    allowance_tokens integer,
    primary key (workspace_id, connector_id)
);
