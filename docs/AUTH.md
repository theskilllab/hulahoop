# Users and sign-in

## Who a request is

Every request resolves to a user in one place, `Authenticator::resolve`
(`crates/server/src/auth.rs`), from the `Authorization: Bearer …` header:

| Credential | Resolves to | Used by |
| --- | --- | --- |
| The server token (`connection.json`) | the oldest user, the *machine account* | runners, the local desktop |
| A session token (`hs_…`) | the person who signed in | desktop or browser over the network |
| A personal API token (`hat_…`) | its owner | scripts, a future MCP server |

Sessions and API tokens are stored as SHA-256 hashes; passwords as PBKDF2-HMAC-SHA256
(600 000 iterations, `ring`). A person who is no longer in the workspace has no access,
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

## Decision: why not `axum-login` + `tower-sessions` + `argon2`

The issue proposed that stack. We chose bearer sessions and `ring` instead:

- `axum-login`/`tower-sessions` are built around browser cookies. The desktop app and
  a future MCP client present a bearer token; cookies would add a second credential
  path (and CSRF handling) beside the bearer one that runners, personal tokens and
  the WebSocket already need.
- `ring` is already in the dependency tree; PBKDF2 at this cost is an accepted
  password KDF. `hash_password`/`verify_password` carry a format prefix, so moving to
  Argon2 later means verifying old hashes and rehashing on sign-in.
- SSO fits behind `resolve`: an OIDC callback (`oauth2`/`openidconnect`) would end by
  creating a session exactly like `/auth/login` does. 2FA and passkeys are a step in
  the sign-in route, not a change to resolution.

This deviates from the issue's starting proposal; the Architect should confirm it.

## Known limits

- GraphQL is POST-only; `GET /graphql?query=…` is not served.
- The server token authorises owner actions from any peer. Behind `--listen` on a
  non-loopback address, treat it as a root password and use HTTPS.
- No password change or reset, and no per-IP rate limit on the sign-in routes yet.
