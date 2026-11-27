-- How many times reviewers sent a pull request back for changes, so the list
-- can say which round of review it is in. Rows fill in on the next sync that
-- reads the reviews.
alter table pull_requests add column changes_requested integer not null default 0;
