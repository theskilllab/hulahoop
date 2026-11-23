-- Lumi, the workspace assistant. The harness and model are a workspace
-- setting; the conversation is workspace-scoped and survives restarts.
alter table workspaces add column lumi_harness text;
alter table workspaces add column lumi_model text;

create table lumi_messages (
  id blob primary key,
  workspace_id blob not null references workspaces(id),
  -- 'user' or 'lumi'
  role text not null,
  body text not null default '',
  -- What the asker was looking at, e.g. "Task HUL-3", so an answer can be
  -- read later and still make sense.
  context text,
  -- A proposal is a JSON object {title, items:[{group,text,action,args}]}.
  -- Nothing in it runs until a person applies it.
  proposal text,
  -- 'open', 'applied' or 'dismissed'; null for a message with no proposal.
  proposal_state text,
  created_at text not null
);

create index lumi_messages_workspace on lumi_messages (workspace_id, created_at);
