-- A skill's name is also its directory in the checkout (`.hulahoop/skills/<name>`),
-- so two skills that differ only in case would overwrite each other's file.
create unique index if not exists skills_workspace_name on skills (workspace_id, lower(name));
