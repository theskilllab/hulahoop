-- Which credential a run started with: a connector's name, `Pi login` or
-- `host login`. A run that quietly used the host login proves nothing about a
-- connector, so the run says which one it was given. Older runs stay unknown.
alter table runs add column credential_source text;

-- The plan a person says a subscription is on, by the plan id the server's
-- catalogue lists. Names only reach the Connectors screen; the fee pre-fills
-- Cost control.
alter table connectors add column plan text;

-- The connector the agent chose when the run was queued. `connector_id` is
-- rewritten at claim to the connector the run actually used (null for the host
-- login), so a re-claim reads the choice from here.
alter table runs add column chosen_connector text;

-- The login a run or a Lumi job was handed, as `{connector, row}`: a refreshed
-- login handed back for anything else is refused.
alter table runs add column credential_refresh text;
alter table runner_jobs add column credential_refresh text;
