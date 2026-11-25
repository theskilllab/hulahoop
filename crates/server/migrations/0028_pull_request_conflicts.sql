-- Whether an open pull request still merges cleanly into its base branch, as
-- GitHub's own conflict check last said: 'clean', 'conflicting', or null while
-- it is unknown (never asked, or GitHub was still computing it).
--
-- GitHub does not move a pull request's `updated_at` when its base branch moves
-- under it, so a conflict can appear without the listing changing. When the
-- check was made is kept beside it, so the sync can ask again after another
-- pull request merged, or after a while, rather than only when the listing moves.
alter table pull_requests add column mergeable text;
alter table pull_requests add column mergeable_checked_at text;
