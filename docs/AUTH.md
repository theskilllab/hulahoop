# Users and sign-in

## Who a request is

Every request resolves to a user in one place, `Authenticator::resolve`
(`crates/server/src/auth.rs`), from the `Authorization: Bearer …` header:

| Credential | Resolves to | Used by |
| --- | --- | --- |
| The server token (`connection.json`) | the oldest user, the *machine account* | runners, the local desktop |
| A session token (`hs_…`) | the person who signed in | desktop or browser over the network |
| A personal API token (`hat_…`) | its owner | scripts, a future MCP server |

Sessions and API tokens are stored as SHA-256 hashes; passwords as Argon2id PHC
strings (`password-auth` 1.0, hashed and verified on `spawn_blocking`). A person who is no longer in the workspace has no access,
whatever credential they still hold.

The machine account cannot be removed or demoted: the server token acts as it, so
an owner who removed it would leave the machine with no one to act as.

## Setting up

- **First owner.** While nobody has a password, `serve` logs a single-use setup link
  (30 minutes). Opening it takes over the existing `you` user, so its issues, comments
  and runs keep their owner.
- **Headless.** `hulahoop user create --owner --name … --email …` (password from
  `--password` or `HULAHOOP_PASSWORD`) does the same without a browser, and
  `hulahoop user setup-link` prints a fresh link when the first one has expired.
- **Registration** is `closed` (default), `invite` or `open`: `--registration` on
  `serve`/`up`, `HULAHOOP_REGISTRATION`, or Settings → Members. An owner can always
  create a single-use invite (7 days).

## Decision: libraries (settled)

- **Passwords:** `password-auth` 1.0 (Argon2id, PHC strings, constant-time verify).
  Not PBKDF2 from `ring`. Unknown emails verify against a dummy hash so a failed
  sign-in costs the same as a wrong password.
- **Sessions and personal API tokens:** our own table. 32 random bytes stored as
  SHA-256, one indexed lookup, an expiry for sessions. Every client sends
  `Authorization: Bearer`; the desktop webview cannot rely on third-party cookies.
- **Not used:** `axum-login` / `tower-sessions` (browser cookies, a second credential
  path). If cookie sessions are added later, use exactly `axum-login 0.18` +
  `tower-sessions 0.14` + `tower-sessions-sqlx-store 0.15` (sqlite); newer versions
  pull in sqlx 0.9, which cannot link beside our sqlx 0.8. Also not `password-auth`
  2.0 (pulls `getrandom` 0.4 and `argon2` 0.6 for no gain) and not JWT (cannot be
  revoked without the same table).
- SSO fits behind `resolve`: an OIDC callback (`oauth2`/`openidconnect`) would end by
  creating a session exactly like `/auth/login` does. 2FA and passkeys are a step in
  the sign-in route, not a change to resolution.

## Known limits

- GraphQL is POST-only; `GET /graphql?query=…` is not served.
- The server token authorises owner actions from any peer. Behind `--listen` on a
  non-loopback address, treat it as a root password and use HTTPS.
- No password change or reset, and no per-IP rate limit on the sign-in routes yet.
