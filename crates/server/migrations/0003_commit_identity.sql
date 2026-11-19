-- Who wrote the commit the loop produced, and where the pull request for it lives.
--
-- The identity columns exist because the run row is the only place a reviewer can
-- look to answer "did the loop commit as itself?": reading it back from the
-- repository after the fact needs a checkout that may already be gone.
alter table runs add column commit_name text;
alter table runs add column commit_email text;
alter table runs add column pr_url text;
alter table runs add column pr_number integer;
alter table runs add column pr_branch text;

-- Where the repo lives on GitHub, so a pull request job needs no discovery step.
alter table repos add column remote_url text;
