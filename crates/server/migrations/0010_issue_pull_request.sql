-- The pull request belongs to the issue, not to a run. A rework pushes new
-- commits to the same branch, and the reviewer's verdict is a review on the same
-- pull request, so one place holds the branch, the number and the url.
alter table issues add column pr_url text;
alter table issues add column pr_number integer;
alter table issues add column pr_branch text;
