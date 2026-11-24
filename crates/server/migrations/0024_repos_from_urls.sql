-- Every repository is now a Hulahoop-owned clone of its url, kept under
-- DATA_DIR/mirrors: a checkout elsewhere on the machine is never borrowed. So
-- `repos.path` goes. A repository attached by path keeps working when that
-- path is usable as a clone url, so it becomes the url where none was set.
--
-- Replays belonged to a review model that no longer exists; their table goes.

update repos
set remote_url = trim(path)
where (remote_url is null or trim(remote_url) = '')
  and path is not null
  and trim(path) <> '';

alter table repos drop column path;

drop index if exists one_active_replay_per_issue;
drop table if exists replays;
