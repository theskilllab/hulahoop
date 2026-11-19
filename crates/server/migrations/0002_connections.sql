-- A connection is a provider the loop can act as on the user's behalf. The
-- credentials are what a GitHub app is: an app id and a private key. Both are
-- stored as vault ciphertext, because a database page is exactly the kind of
-- place a credential should never be readable from.
create table connections (
    id blob primary key,
    provider text not null unique,
    app_id_ciphertext text not null,
    private_key_ciphertext text not null,
    created_at text not null,
    updated_at text not null
);
