-- One table for every credential the loop can act with, because the second one
-- arrived the day after the first: a GitHub app is an app id and a key, an agent
-- subscription is a token or a credentials file. The shape below is the shape they
-- share — a provider, a kind, and a sealed payload nobody reads in a backup.
create table connectors (
    id blob primary key,
    provider text not null unique,
    kind text not null,
    -- sealed JSON: {"app_id":…, "private_key":…} for a github app,
    -- {"secret":"…"} for an api key, {"file":"…","contents":"…"} for a subscription
    secret_ciphertext text not null,
    -- what the screen shows beside the provider: an app id, an account name, "api key"
    label text,
    created_at text not null,
    updated_at text not null
);

-- The github app already stored by 0002 moves across unchanged; sealing happened
-- before it was written, so the ciphertext is copied, not re-encrypted.
insert into connectors (id, provider, kind, secret_ciphertext, label, created_at, updated_at)
select id,
       provider,
       'github_app',
       '{"app_id":' || app_id_ciphertext || ',"private_key":' || private_key_ciphertext || '}',
       slug,
       created_at,
       updated_at
from connections;

drop table connections;
