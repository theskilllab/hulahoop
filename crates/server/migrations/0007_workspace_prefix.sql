-- The workspace prefix used to be the literal 'WS'. It should read like a key: a
-- workspace named `Local` files its project-less issues as `LOC-1`.
update workspaces
set default_prefix = upper(substr(replace(name, ' ', ''), 1, 3))
where default_prefix = 'WS';
