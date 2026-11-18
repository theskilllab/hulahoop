# Hulahoop

**The acceptance loop that keeps coding agents honest.**

Hulahoop is a desktop-first, local-first workspace for running autonomous coding agents through one opinionated loop:

> criteria → failing tests (frozen) → implementation → review → approve → merge

It is intentionally small. One workflow, one repo per run, one active run per issue, one human approver, almost no settings.

- Domain: `hulahoop.dev`
- Status: **runnable** — the server, runner, CLI and desktop shell are built, and the
  end-to-end loop reaches `done` with real `pi` agents (see
  [18. What is built today](#18-what-is-built-today))
- First project: Hulahoop itself

---

## 1. Product principles

1. **One workflow.** No workflow builder, custom statuses, or plugin SDK.
2. **Tests first, frozen, and red.** An issue is not ready until its acceptance criteria are an executable suite that fails at the base revision. The suite is written by an agent that never sees the implementation, and the Implementer cannot edit it.
3. **Evidence over claims.** A run is done when the diff, tests, and log say so.
4. **Local-first.** The desktop app owns the local server and runner. No account, cloud, or tunnel required.
5. **Durable runs.** Cancellation, restart, timeout, and cleanup are part of the core loop.
6. **Dogfood from day one.** After the bootstrap, Hulahoop manages its own development.
7. **Multiple projects and checkouts from day one.** The schema models workspaces, projects, repos, and run checkouts even when local mode has one user.
8. **Multiple agents from day one.** Handoffs are the product. v0 runs a Test Author, an Implementer, and a Reviewer as three separate agent configurations, each with its own prompt and its own view of the work.
9. **Open source by default.** The core is MIT-licensed, local-first, and buildable without a hosted service.
10. **Few settings.** Workspace, project, repo, and agent choices; no workflow settings.

---

## 2. Smallest iteration: Hulahoop 0

The first version exists to answer one question:

> Can Hulahoop take an issue from “ready” to “approved” using its own agents, with a suite that provably fails before the implementation and passes after it?

### In scope

- One local user, with a schema that already supports multiple workspaces, projects, and repos.
- First project: the Hulahoop repo.
- Two adapters: Pi first, Claude Code second.
- Three roles: Test Author, Implementer, Reviewer.
- Three agents: Pi Test Author, Pi Implementer on a different model, Claude Code Reviewer. The human approves and merges.
- One frozen test suite per issue, authored before any implementation and replayed at the base revision.
- One repo contract: `flake.nix` and `justfile` with `setup` and `test` recipes, plus the `.envrc` the runner generates. No per-repo command settings.
- A per-run sandbox: a scratch `HOME` and a clean environment in v0, extended to a read-only host outside the checkout in M1.
- Local notifications when an issue needs a human: `in_review`, `blocked`, and `merging`.
- SQLite database.
- Local server on `127.0.0.1`.
- Desktop app with:
  - project picker
  - issue list
  - issue detail
  - create/edit issue
  - acceptance criteria field
  - suite status: red at base, green at head
  - “Run” button per role
  - live run log
  - diff view
  - comments, which carry a blocking question and its answer
  - approve / request changes
  - cancel run
- jj workspace creation and cleanup.
- Git worktree fallback.
- Run recovery after server restart.

### Out of scope

- Multi-user, teams, invitations, RBAC.
- Cloud hosting, remote runners, public ingress.
- GitHub/GitLab PR creation.
- QA agent; the Test Author ships in v0, QA in M2.
- Mobile, chat integrations, desktop voice input.
- Custom workflows, labels, plugins, model marketplace.
- Billing, budgets, usage dashboards.
- More than two adapters.
- Windows desktop builds; v0 targets Linux and macOS, where `nix` and POSIX process groups already exist.

### The one loop

```text
backlog
  └─ ready            acceptance criteria written, human approved them
      └─ tests_in_progress  Test Author produces a test-only diff from base;
          │                 Hulahoop replays it red at base
          └─ tests_ready  freezeSuite pins the `tests/<issue>` bookmark;
              │           every authored test fails there (unfreezeSuite sends it back)
              └─ in_progress  Implementer branches from that bookmark
                  └─ in_review  suite green at head; diff, log, and replay rows available
                      ├─ in_progress  changes requested; the human starts the next run
                      └─ merging      human approved; landing replay on the current base
                          ├─ in_progress  the replay turned red or timed out
                          └─ done     merge commit recorded

blocked: needs human input; the question is pinned; resumeIssue writes the answer and returns to status_before_blocked
cancelled: the human abandoned the issue; cancelRun instead stops the process and leaves the issue where it was
```

No labels in Hulahoop 0. The frozen suite and the status carry the gate.

### Acceptance criteria

Criteria are the suite's source of truth, so they have a fixed shape. One line each:

```text
- <observable behavior> when <condition>
```

Checks the `ready` gate enforces:

- each criterion names something observable: output, exit code, API response, file content
- each criterion is independently checkable, and gets exactly one test whose name carries its position: `ac1` for the first, `ac7` for the seventh
- `freezeSuite` refuses a criterion that is empty, generic (`works`, `is fast`, `handles errors`), names a function, module, or data structure inside the implementation, or has no test named after it

The name convention is what makes the gate mechanical: the freeze gate parses the authored JUnit report, matches `ac<position>` at a token boundary (`(^|[^A-Za-z0-9])ac12([^0-9]|$)`), requires exactly one match per criterion, requires each match to fail at base, and writes it into `criteria.test_name`. The boundary matters: a plain substring test lets `ac1` match `ac10`, so a ten-criteria suite would silently accept a missing test. Test names are also required to be distinct, since the implementation column of the same table carries them. An Implementer rename would break the green side of the proof, so test names sit inside the frozen diff.

The Test Author prompt carries the same rules plus three of its own: assert behavior over internals, one test per criterion, and every test must fail against an untouched base revision.

### Landing a change

The human merges. Hulahoop prepares the merge and verifies the result:

1. `approveIssue(runId, reviewedHead, diffSha, acknowledgeFlags)` records the approval against the exact head and diff the human saw, and moves the issue to `merging`. Starting a new run on the issue voids it.
2. In `merging`, Hulahoop rebases the approved head onto the freshly fetched base, writes `landing/<issue>/<attempt>` at the rebased revision, checks that the approved head is an ancestor of it, and runs the landing replay, `direnv exec . just test`. `recordMerge` in step 4 verifies that bookmark, so the tree a human merges is the tree that was replayed.
3. A rebase that turns the suite red returns the issue to `in_progress` with a comment naming the failing tests.
4. The human merges `landing/<issue>/<attempt>`, and `recordMerge(mergeRevision)` closes the issue as `done` with the merge commit on the issue, so the loop ends with a revision anyone can find. The guard reads the landing replay's own row, so a green replay from an earlier attempt cannot unlock this merge.

In v0 the merge is a human action with a Hulahoop-verified pre-merge replay. Hulahoop performing the merge is M3.

---

## 3. Architecture

```text
┌──────────────────────┐        GraphQL        ┌──────────────────────┐
│ Desktop              │ ────────────────────▶ │ Server               │
│ Tauri + React        │   queries/mutations   │ Rust + Axum          │
│                      │ ◀──────────────────── │ async-graphql        │
│                      │   subscriptions       │ SeaORM + SQLite      │
└──────────────────────┘                       └──────────┬───────────┘
                                                          │
                                                          │ internal HTTP/WS
                                                          │ claim / heartbeat / logs
                                                          ▼
                                               ┌──────────────────────┐
                                               │ Runner               │
                                               │ Rust                 │
                                               │ jj workspace         │
                                               │ agent process        │
                                               └──────────┬───────────┘
                                                          │
                                                          ▼
                                               ┌──────────────────────┐
                                               │ Agent CLI            │
                                               │ Pi / Claude Code     │
                                               └──────────────────────┘
```

### Components

**`hulahoop-server`**
- Rust, Axum, `async-graphql`, SeaORM.
- Owns the state machine, database, approvals, run history, the log files, and the artifact directories.
- The only process that writes SQLite. The runner reports facts over HTTP, so there is one writer and no cross-process lock contest.
- Owns `repo_locks`. The runner asks for and releases a lock over the internal API, so every lock has one owner and one expiration.
- Binds `127.0.0.1` on a random port.
- Writes a connection file with `{ port, token }` for the desktop app.
- Exposes GraphQL for the desktop.
- Exposes a small internal API for the runner.

**`hulahoop-runner`**
- Rust.
- Connects outbound to the server.
- Claims one run at a time.
- Creates a checkout.
- Enters the repo contract with `direnv exec` for setup, the agent, and all three replays (base, head, landing).
- Starts the agent CLI with the direnv environment already exported.
- Posts events and artifacts to the internal API; the server writes the log file.
- Kills the process group on cancel or timeout.
- Cleans the checkout on completion.

**`hulahoop-desktop`**
- Tauri 2 + React + TypeScript.
- Vite for the renderer; Tauri CLI for packaging.
- `urql` + `graphql-codegen`; subscriptions over `graphql-ws`.
- Spawns sidecar `hulahoop-server` and `hulahoop-runner` binaries in local mode.
- Opens the UI against the local server.

**`hulahoop` CLI**
- Bootstrap and debugging helper.
- `hulahoop server`, `hulahoop runner`, `hulahoop issue new`, `hulahoop suite replay <issue>`.
- Not the primary UI.

### Why desktop first

- Direct access to local repos, jj, Git credentials, and agent CLIs.
- No public ingress, TLS, or account system.
- One machine to debug.
- The server and runner stay headless, so a remote mode can reuse them later.

The desktop app is a client. It never owns the state machine.

---

## 4. Stack decisions

| Area | Decision |
| --- | --- |
| Server | Rust, Axum |
| API | GraphQL via `async-graphql` |
| Subscriptions | `graphql-transport-ws` or SSE |
| ORM | SeaORM |
| Local DB | SQLite with WAL |
| Remote DB | Postgres, later |
| Desktop | Tauri 2 + React + TypeScript |
| Desktop build | Tauri CLI + GitHub Actions |
| GraphQL client | `urql` + `graphql-codegen` (fallback: Apollo) |
| VCS | jj workspaces first, git worktree fallback |
| Agent adapter | Pi first, Claude Code second |
| Agents | Multiple, role-scoped from v0 |
| License | MIT (recommended) |
| Repo contract | `flake.nix` + `justfile` (`setup`, `test`), a generated `.envrc`, executed via `direnv exec` |
| Shell entry | `direnv` + `nix-direnv`; cached dev shell per `flake.lock` |
| Toolchain | Nix flake per repo; `flake.lock` recorded per run |
| Test report | JUnit XML at `.hulahoop/junit.xml` |
| Test gate | suite replays red at base, green at head; runner enforces both |
| Process control | `tokio::process`, process group kill |
| Logs | append-only file + in-memory tail |

### GraphQL boundary

GraphQL is for the desktop client:

- queries for projects, issues, runs, agents
- mutations for issue create/update, run start/cancel, approve, comment
- subscriptions for `issueChanged`, `replayChanged`, `runChanged`

The runner protocol stays plain and internal:

```text
POST /internal/runs/claim
POST /internal/runs/:id/heartbeat
POST /internal/runs/:id/events
POST /internal/runs/:id/artifacts
POST /internal/runs/:id/verdict
POST /internal/runs/:id/complete
GET  /internal/runs/:id/spec
POST /internal/replays/claim
POST /internal/replays/:id/complete
POST /internal/replays/:id/ancestry
POST /internal/preflight/claim
POST /internal/preflight/:id/report
POST /internal/repos/:id/lock
POST /internal/repos/:id/unlock
```

`verdict` is how a Reviewer reports `approved` or `changes_requested` with its body; the server writes the `reviews` row and fires the transition. An agent that asks a question posts it as a `question` event on the same stream, which the server turns into a pinned comment. `ancestry` is how a landing replay reports that the approved head is contained in the rebased revision, so `recordMerge` never inspects a repository. `preflight` is a runner job with a checkout and no run: the runner probes the adapter in that repo's dev shell and posts the outcome, which lands in `agent_preflight`.

Every internal route and the log socket require `Authorization: Bearer <token>`, compared in constant time against the per-launch token in the connection file. The token is never passed to an agent, so a run that reads its own environment cannot claim work. v0 keeps the connection file under `$DATA` with mode `0600`, which an agent could still read from disk; that gap closes when M1 confinement lands, and §14 records it as an accepted risk rather than an oversight.

Run logs stream over a dedicated WebSocket/SSE endpoint. GraphQL subscriptions carry structured invalidation, not every token.

### SeaORM boundary

- SeaORM for CRUD and simple relations.
- DataLoader batching for nested GraphQL fields.
- Raw SQL for dashboards, cost rollups, and hot lists.
- Run migrations with `sea-orm-migration`.
- UUIDv7 IDs.

---

## 5. Data model

```text
users
  id, name, email, created_at

workspaces              top-level container; one local row in v0
  id, name, slug, created_at

workspace_members
  workspace_id, user_id, role

projects
  id, workspace_id, name, created_at

repos                   a project can have more than one
  id, project_id, name, path, vcs, base_revision, contract, contract_version,
  fetched_at, created_at
                        contract: nix+just (v0), validated at add time
                        contract_version: the Hulahoop build's CONTRACT_VERSION,
                          nothing is declared by the repo
                        base_revision: resolved from the default branch after a
                          fetch; fetched_at records when that happened

agents
  id, workspace_id, name, adapter, model, created_at

agent_preflight       the last probe of one adapter in one repo
  agent_id, repo_id, ok, detail, checked_at
                        keyed by the pair, because the probe runs inside that
                          repo's dev shell: one toolchain cannot answer for another

project_agents        role: test_author | implementer | reviewer
  project_id, agent_id, role

issues
  id, project_id, repo_id, title, body, status, priority,
  status_before_blocked, pinned_comment_id, run_cap_cleared_at,
  attempt, suite_seq, run_count, current_run_id, tests_revision,
  tests_bookmark, frozen_paths, tests_frozen_at,
  merge_revision, merged_at, created_at, updated_at
                        status_before_blocked: where resumeIssue returns the issue
                        pinned_comment_id: the agent's open question while blocked
                        run_cap_cleared_at: human override for the runs-per-issue cap
                        attempt: increments per implementation run, names every
                          implementer bookmark and worktree branch of that attempt
                        suite_seq: increments per authored suite, so archives
                          survive several thaws inside one attempt
                        current_run_id: the implementer run a human may approve;
                          set when an implementer run completes, cleared by a thaw
                        frozen_paths: the files the authoring run wrote, written by
                          freezeSuite and the set every later diff is checked against
                        tests_revision: the commit `tests/<issue>` points at
                        run_count: rows in `runs`, what the cap counts
                        merge_revision: the commit recordMerge accepted

criteria              acceptance criteria, one row each, numbered from 1
  id, issue_id, position, text, test_name
                        test_name: filled by the freeze gate from the authored
                          report; one criterion maps to exactly one test

checkouts              one per active run, replay, or preflight
  id, project_id, repo_id, owner_kind, owner_id, provider, name, path, state,
  created_at, cleaned_at
                        owner_kind: run | replay | preflight; owner_id points at
                          that row, so the base replay, the reviewer, the landing
                          replay, and a preflight each get their own checkout
                        state: creating | ready | cleaning | removed

runs
  id, issue_id, repo_id, agent_id, role, attempt, base_bookmark, base_revision,
  head_revision, head_bookmark, parent_run_id, runner_id, claimed_at,
  last_heartbeat_at, pid, pgid, toolchain_lock, prompt_revision, sandbox,
  env_keys, artifact_dir, diff_sha, flags, status, started_at, finished_at,
  exit_code, error
                        role: test_author | implementer | reviewer
                        attempt: copied from `issues.attempt` for an implementer
                        base_bookmark: the bookmark the checkout was created from
                        toolchain_lock: sha256(flake.lock) at run start
                        parent_run_id: set when the run stacks on a prior attempt
                        head_bookmark: issue/<id>/head/<attempt>, the handle
                          a human and the landing replay resolve
                        prompt_revision: app version plus sha256 of the
                          embedded prompt set
                        sandbox: profile hash; env_keys: names passed, never values
                        diff_sha: sha256 of diff.patch at completion, what
                          approveIssue checks
                        flags: touched_contract | large_diff | artifact_limit,
                          read by the approve dialog
                        artifact_dir: $DATA/runs/<run-id>/

replays               one row per suite execution Hulahoop verifies itself
  id, issue_id, run_id, approval_id, kind, attempt, revision, tests_bookmark,
  status, ancestry_ok, artifact_dir, created_at, finished_at
                        kind: base | head | landing
                        run_id: null for a landing replay, which belongs to
                          the issue rather than to an agent run
                        approval_id: set on a landing replay, so the gate and
                          recordMerge read the replay of this approval only
                        attempt: the implementation attempt being verified
                        tests_bookmark: the authored suite the replay applied,
                          so a replay can never be mistaken for another version
                        ancestry_ok: written by the landing replay after it checks
                          that the approved head is an ancestor of the rebased
                          revision; recordMerge reads it instead of touching jj
                        status: queued | running | succeeded | failed | timed_out;
                          a replay has no agent, so cancelled does not apply

reviews
  id, issue_id, run_id, verdict, body, created_at

comments
  id, issue_id, author_type, author_id, body, created_at

approvals             bound to the exact diff the human reviewed
  id, issue_id, run_id, head_revision, diff_sha, landing_bookmark,
  acknowledged_flags, approved_by, voided_at, created_at
                        voided_at: set when a new run starts on the issue or the
                          landing replay turns red, so a stale approval can
                          never be read as the one that shipped
                        landing_bookmark: landing/<issue>/<attempt>, the rebased
                          revision the landing replay actually verified

repo_locks            one advisory row per repo, written only by the server
  repo_id, holder, acquired_at, expires_at

run_events
  id, run_id, seq, kind, payload, created_at
```

Local mode seeds one user, one workspace, and one membership. The rows are real, so remote mode and multiple projects do not require a schema rewrite.

Logs live in `$DATA/runs/<run-id>/`; the database stores metadata and `artifact_dir`, never log text. `Run.log` is the file, and the server keeps a bounded in-memory ring of the last 500 events per running run so the screen does not re-read the file on every tick.

```text
$DATA/runs/<run-id>/
  agent.jsonl        the full agent event stream, appended by the server
  events.jsonl       the run event stream
  diff.patch         the diff under review

$DATA/replays/<replay-id>/
  junit.xml          the JUnit report the gate parses
  just-test.log      the full `just test` output for that replay
```

The server writes every file here; the runner posts the bytes. One writer keeps the log and the JUnit reports consistent with `runs.status`, and the reviewer reads the same files the gate read. A replay is its own row and its own directory, so red at base, green at head, and the landing replay stay distinguishable.

### Run states

```text
queued → running → succeeded
                 ├→ failed
                 ├→ cancelled
                 └→ timed_out
```

`run_events` is unique on `(run_id, seq)`. `agent.jsonl` on disk is the authoritative stream for agents and reviewers; the in-memory ring serves the live tail, and a finished run streams from the file.

A run is never deleted. A new attempt is a new run.

`replays` share the state machine with one difference: a replay has no agent, so `succeeded` means `just test` exited 0 and `failed` means it exited non-zero. The gate reads `replays.status` and `junit.xml` together: `succeeded` is green, `failed` is red, and `timed_out` is neither, so a timed-out base replay is retried and a timed-out landing replay sends the issue back to `in_progress` naming the timeout.

The database stores statuses as the lowercase names above; GraphQL exposes them as `SCREAMING_SNAKE` enum members (`TESTS_IN_PROGRESS` ↔ `tests_in_progress`), one mapping table in code.

### Issue transitions

The server owns every transition. A mutation or a run completion event fires it; the desktop asks and re-renders. Guards are server-side, so a client cannot skip the gate.

| From | Event | Guard | To |
| --- | --- | --- | --- |
| `backlog` | `approveCriteria` | every criterion matches the required shape | `ready` |
| `ready` | `startRun(role: test_author)` | criteria valid, agent bound to the role, no run queued or running on the repo | `tests_in_progress` |
| `tests_in_progress` | `startRun(role: test_author)` | no run queued or running, reached by a thaw or a rejected freeze | `tests_in_progress` |
| `tests_in_progress` | authoring run completes | writes `tests/<issue>/cand/<run-id>` and queues the base replay | `tests_in_progress` |
| `tests_in_progress` | base replay completes | it ran the candidate suite against `repos.base_revision` | `tests_in_progress` |
| `tests_in_progress` | `freezeSuite(issueId)` | base replay present, every authored test fails in it, each criterion maps to exactly one test name, no run queued or running | `tests_ready` |
| `tests_in_progress` | `freezeSuite(issueId)` rejected | an authored test passes at base, or a criterion has no test | `tests_in_progress` |
| `tests_in_progress` | authoring run fails | `runs.error` records why, nothing is frozen yet | `ready` |
| `tests_ready` | `startRun(role: implementer)` | suite frozen, no run queued or running on the repo | `in_progress` |
| `tests_ready` | `unfreezeSuite(reason)` | human, archives `tests/<issue>` as `tests/<issue>/archive/<suite_seq>` and clears `current_run_id` | `tests_in_progress` |
| `in_progress` | `startRun(role: implementer, parentRunId)` | no run queued or running; the rework run branches from that run's head bookmark | `in_progress` |
| `in_progress` | implementer run completes | head replay green, frozen test files unchanged, sets `current_run_id` | `in_review` |
| `in_progress` | implementer run fails | `runs.error` records why; the issue stays put so the human can start another attempt | `in_progress` |
| `in_review` | `startRun(role: reviewer)` | `current_run_id` set and that run has a head bookmark | `in_review` |
| `in_review` | reviewer verdict `changes_requested` | review row written, body quotes the failing tests | `in_progress` |
| `in_review` | reviewer verdict `approved` | review row written, the approval still needs a human | `in_review` |
| `in_review` | `requestChanges(body)` | human, comment recorded | `in_progress` |
| `in_review` | `approveIssue(runId, reviewedHead, diffSha, acknowledgeFlags)` | `runId` is `current_run_id`, that run has an `approved` review, head and diff match it, every flag acknowledged | `merging` |
| `merging` | landing replay turns red or times out | failing test names in the replay report, the approval is voided | `in_progress` |
| `merging` | `recordMerge(mergeRevision)` | a green landing replay for this approval exists, `ancestry_ok` is true | `done` |
| `blocked` | `resumeIssue(issueId, answer)` | writes the answer comment, clears `pinned_comment_id`, `status_before_blocked` is one of `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | `status_before_blocked` |
| `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | agent ends its run with a question | comment recorded and pinned | `blocked` |
| `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | `cancelRun(runId)` | process group killed; the issue keeps its status and loses its active run | unchanged |
| any state except `done` and `cancelled` | `abandonIssue(reason)` | human | `cancelled` |

`done` and `cancelled` are terminal. `done` is reachable only through `recordMerge` behind a green landing replay. `cancelled` means the human abandoned the issue, which is why it has its own mutation: cancelling a run stops the process and leaves the issue where it was, so a misclick costs one run and never the issue. `blocked` is not an active state for the purposes of this table: it holds no run, and every row that leaves it is a human action.

Two events can arrive for one run, so precedence is explicit: a run whose final message is a question fires the `blocked` row and does **not** fire the completion row, because "completes" in that table means a run that finished without a question. A base replay queued before a block still lands while the issue is `blocked` and leaves it `blocked`.

The Reviewer works while the issue sits in `in_review`, so its own row leaves the status alone until a verdict lands; an `approved` verdict still waits for the human, which is what the independence in §3 buys. An agent asks a question by ending its run with that question, so `blocked` never holds a live process: the run completes, `pinned_comment_id` points at the question, and the next run's prompt carries the human's answer. A question asked while the issue is already blocked leaves `status_before_blocked` alone, so `resumeIssue` cannot land back on `blocked`.

The rework loop is the same `startRun` reached from `in_progress` with `parentRunId`, which is why a rejected diff, a changes-requested verdict, and a red landing replay all converge on `in_progress` instead of forking into their own states.

Three rules live in the schema, so a race cannot violate them:

- `create unique index one_active_run on runs(issue_id) where status in ('queued','running')` gives one queued or running run per issue. A second `startRun` fails with "a run is already active".
- `create unique index one_active_run_per_repo on runs(repo_id) where status in ('queued','running')` gives one running agent per repo. Two agents in one repo would fight the same jj op log and the same `cargo`/`pnpm` cache, so the second run queues behind the index instead of behind a sentence in the risk table. Parallel work across projects still works; parallelism inside one repo arrives when replays get a build cache of their own.
- `startRun(issueId, agentId, role)` rejects an agent that is not bound to that role in `project_agents`, so the three-role handoff cannot be reassigned by a client.

### Defaults

Constants in code, not settings. The UI shows them where they bite:

| Limit | Value |
| --- | --- |
| runner heartbeat | 5s |
| runner declared dead after | 30s without a heartbeat |
| agent wall clock, `just setup` excluded | 30 min per role |
| `just setup` | 20 min |
| one `just test` replay | 15 min |
| runs per issue | 12 rows, which covers three roles across four attempts; the Run button disables until a human calls `clearRunCap` |
| GraphQL request | 30s |
| SQLite `busy_timeout` | 5s |
| log rotation | at 50 MB a run keeps its first and last 50 MB (`agent.jsonl`, `agent.1.jsonl`); metadata retained forever |
| diff size | 2000 changed lines, then the run is flagged for human review |
| run artifact directory | 500 MB, then the run fails with status `failed` and the `ARTIFACT_LIMIT` flag |
| connection file mode | `0600`, read by the desktop at launch |

A run that exceeds its time limit is killed by process group and recorded as `timed_out`. A missing heartbeat triggers the same kill path from the server side, so a crashed runner cannot leave agents running. The artifact limit is not a time limit: that run ends as `failed` with `ARTIFACT_LIMIT` on it, because its evidence is unusable.

---

## 6. Checkouts

A checkout is the per-run working copy: a jj workspace or a git worktree. It is separate from a Hulahoop workspace, which is the top-level container for projects.

Hulahoop detects `.jj` and uses a jj workspace per checkout.

```bash
jj workspace add \
  --name issue-<id>-<owner-id> \
  -r <base> \
  $DATA/checkouts/<project>/<issue-id>/<owner-id>
```

`$DATA` is the data root: `~/.local/share/hulahoop` by default, `~/Library/Application Support/hulahoop` on macOS, overridable with `HULAHOOP_DATA`. Every path in this document is relative to it.

`<base>` is `repo.base_revision` for the Test Author, the `tests/<issue>` bookmark for the Implementer, and the implementer's `issue/<id>/head/<attempt>` bookmark for the Reviewer — the review happens against the tree that contains the implementation. `jj workspace add -r` takes a bookmark name, so the workspace opens at whatever that bookmark points at when it is created.

Rules:

- One checkout per active run, replay, or preflight, keyed by `(owner_kind, owner_id)`. The base replay, the implementer, the reviewer, a landing replay, and a preflight never share one, and a landing replay and a preflight each have a checkout with no run behind it.
- A base replay runs the candidate suite against the base revision: the checkout opens at `repos.base_revision` and the authored test files are restored from `tests/<issue>/cand/<run-id>` with `jj restore --from <that bookmark> -- <frozen paths>`. Without that step the base replay would run an empty diff and prove nothing, because the authored tests do not exist at base until they are applied.
- A landing replay runs the approved head rebased onto the freshly fetched base, and writes the result to `landing/<issue>/<attempt>`. That bookmark is the revision `recordMerge` verifies, so the tree a human merges is the tree that was replayed.
- One repo-level lock around `workspace add`, `workspace forget`, and `git fetch`, acquired through `POST /internal/repos/:id/lock`. The row lives in `repo_locks`, the server is its only writer, and a holder that stops renewing it loses the row at `expires_at`.
- Fetch once per repo, not once per run. A fetch resolves the default branch and writes it to `repos.base_revision` with `repos.fetched_at`; that pair is "the current base" everywhere the spec says it. A fetch runs at add-repo time, when the human asks for it on the project screen, and before each landing replay, because the rebase has to target the branch as it is now. `recordMerge` then sets `repos.base_revision = mergeRevision`.
- The Implementer may not touch frozen test files; the runner rejects a run whose diff reaches them, and the Reviewer sees that guard in the log.
- Cleanup: `jj workspace forget issue-<id>-<owner-id>`, then remove the directory.
- On cancel or crash, cleanup runs on the next server start.
- `.hulahoop/` stays out of the diff through a fileset exclusion on every diff command (`~glob:.hulahoop/**`), which works in colocated, git, and pure jj repos alike. Where a `.git` directory exists, the runner also writes that pattern into `.git/info/exclude` so `git status` in the checkout stays clean for the agent.
- The generated `.envrc` needs the same treatment, and only when the runner generated it: the runner creates the file, writes it into `.git/info/exclude`, and excludes it from the diff because it wrote the contents itself. A committed `.envrc` is tracked, appears in the diff like any other file, and raises `TOUCHED_CONTRACT` — the exclusion never hides a file the repo owns.

### Bookmarks are the handles

A jj commit hash changes whenever the agent amends or rewrites its work, so a stored hash becomes a stale pointer to nothing. Every revision Hulahoop keeps a handle on is a bookmark:

| Bookmark | Set when | Points at |
| --- | --- | --- |
| `tests/<issue>/cand/<run-id>` | an authoring run completes | that run's tests, the input to the base replay and to `freezeSuite` |
| `tests/<issue>` | `freezeSuite` | the frozen suite, the tree the Implementer branches from |
| `tests/<issue>/archive/<suite_seq>` | `unfreezeSuite` | the suite that was replaced, numbered by `issues.suite_seq` so several thaws in one attempt keep their own archive |
| `issue/<id>/head/<attempt>` | an implementer run completes | the diff under review and the base of the next attempt |
| `landing/<issue>/<attempt>` | a landing replay succeeds | the rebased revision the merge must contain |

`<attempt>` is `issues.attempt`, which increments each time an implementer run starts, and `<suite_seq>` increments each time a suite is frozen. Only implementer runs write a head bookmark, so a reviewer run cannot overwrite the tree it is reviewing, and only a green landing replay writes a landing bookmark, so `recordMerge` never verifies a red tree. `issues.tests_bookmark` and `runs.head_bookmark` store the names; `tests_revision` and `head_revision` store the commit each handle resolved to at the moment it was written, for display and for the approval pin. Anything that has to survive an agent's rewrite — a branch, a replay, a rebase — resolves the bookmark instead of replaying a stored hash. A merged commit and a fetched base are not handles: they are fixed points that no agent can move.

An authoring run writes its own bookmark at completion, so the tests it wrote survive workspace cleanup between authoring and freeze. That window used to have no handle at all.

Fallback when the repo has no `.jj`:

```bash
git worktree add -b issue/<id>/<kind>-<owner-id> <path> <base>
git worktree remove --force <path>
```

The branch keeps the run's commits alive after `worktree remove`; a detached worktree would drop them. The branch is named after the checkout's owner rather than after the attempt, because the author, the implementer, the reviewer, and each replay all need their own branch in the same issue and a shared name fails on the second worktree.

Interface:

```rust
trait CheckoutProvider {
    fn create(&self, issue: &Issue, base: &str) -> Result<Checkout>;
    fn diff(&self, checkout: &Checkout) -> Result<String>;
    fn cleanup(&self, checkout: &Checkout) -> Result<()>;
}
```

Agents get `VCS=jj` and a `jj` skill. In colocated repos, Git commands keep working. In pure jj repos, the prompt teaches `jj st`, `jj diff`, `jj new`, and `jj bookmark`; pushing stays with the human in v0.

### Run sandbox

The checkout isolates the change; the sandbox isolates the agent from the host. Every run gets:

- a scratch `HOME` at `$DATA/home/<adapter>`, one per adapter, so the adapter's login and caches survive cleanup instead of dying with the checkout, and nothing lands in the user's real home
- a clean environment: `env_clear()` on spawn, then an allowlist of `PATH` (from the direnv export), `HOME`, `TMPDIR`, `VCS`, `NO_COLOR`, the named `HULAHOOP_*` variables the run needs, and that adapter's own model credentials. The launching shell contributes nothing else, and the server token is never on the list
- read-write access to the checkout, the scratch `HOME`, the nix store, and the per-repo build cache
- no inherited access to `~/.ssh`, `~/.aws`, `~/.config`, `~/.git-credentials`, or the rest of `$DATA`
- no remote credentials, so a `git push` or `jj git push` inside a run fails at the network layer

The scratch `HOME` is seeded with the adapter's own credential file, `~/.claude/.credentials.json` for Claude Code or `~/.pi` for Pi, and `ANTHROPIC_API_KEY` is passed when the user exports one. Nothing else from the user's home is copied. Because the scratch home outlives the checkout, a Reviewer logs in once and every later run of that adapter reuses it.

In v0 that environment and the scratch `HOME` are the boundary: an agent inherits nothing from the shell, and every credential it could use had to be put there on purpose. Process-level confinement arrives in M1 — Linux under `bwrap`, macOS under `sandbox-exec` with a generated profile — where the same allowlist becomes a filesystem policy: read-write on the checkout, the scratch home, the nix store, and the build cache, read-only on the host, and nothing else visible at all. The profiles ship with the app rather than living in the repo they are meant to contain, and a machine missing the platform tool refuses to start the run instead of running unbounded.

Both versions resolve the dev-shell environment once per repo, before the sandbox exists: the runner runs `direnv export json` in the checkout, takes `PATH` and the `HULAHOOP_*` names it needs, and then starts `env -i <that allowlist> direnv exec . bwrap …`. No direnv runs inside the confined tree, so there is no order to get wrong.

The sandbox record is prepared at checkout creation: `HOME=<scratch> direnv allow` writes the allow record into the scratch home, and the flake entry passes `--extra-experimental-features 'nix-command flakes'` so a user-level `nix.conf` is not required.

The run stores the profile hash as `runs.sandbox` and the passed variable names as `runs.env_keys`.

---

## 7. Repo contract

Hulahoop does not guess how a repo builds and tests itself. A supported repo declares two files, and Hulahoop runs a fixed recipe set against them:

```text
flake.nix      required; pinned toolchain and dependencies, flake.lock is the provenance
.envrc         `use flake`; the shell entry point, cached by nix-direnv.
               Generated when absent
justfile       required; recipes: setup, test
```

| Recipe | Contract |
| --- | --- |
| `just setup` | idempotent provisioning; safe to re-run; the only recipe allowed to need the network |
| `just test` | the whole suite; exit 0 means green; writes JUnit XML to `.hulahoop/junit.xml` |

Optional recipes, used when present: `just fmt-check`, `just lint`, `just ci`. When `just ci` is present it runs `just test` itself, so the JUnit report the gate reads always exists afterwards.

Shell entry goes through direnv, so the command Hulahoop runs is the command a human types:

```bash
direnv exec . just setup
direnv exec . just test
direnv exec . pi -p --mode json --model <model>
```

`nix-direnv` caches the evaluated dev shell in the store and a GC root, so the first entry for a given `flake.lock` costs seconds and every later entry costs milliseconds, across all checkouts of that repo. One wrapper covers setup, the agent, and all three replays, and there is no long-lived subshell to keep alive.

For agents this is the whole integration: `direnv exec .` exports the environment once, and their own `cargo`, `pnpm`, or `pytest` calls inherit it, so `cd` into the checkout is enough.

Rules:

- `contract_version` is a constant in the Hulahoop build, `CONTRACT_VERSION = "26.11"` (year.month of the recipe set). Preflight compares the repo against it and prints the migration when they differ: "repo needs contract 26.11: add the `lint` recipe or drop it from `ci`". The version tracks the recipe set, so a bump ships whenever the set changes, and a repo that matches nothing is refused rather than guessed at.
- Prerequisites are `nix` with flakes, `direnv`, and `nix-direnv`. Preflight runs `nix flake metadata --no-write-lock-file`, `just --list`, and `direnv version`, then rejects the repo with a one-line reason. A repo missing `flake.nix` or `justfile` is unsupported in v0, and the desktop names the file to add.
- `.envrc` is generated when absent: at checkout creation the runner writes `use flake` and runs `direnv allow`, which applies per path, so each checkout allows its own. A committed `.envrc` is used as it stands.
- The runner records `sha256(flake.lock)` as `runs.toolchain_lock`, so a green result is attributable to a toolchain.
- Frozen test files are a hard reject: a diff that touches one ends the run. `flake.nix`, `flake.lock`, `justfile`, and a committed `.envrc` are a flag instead: the run completes, `TOUCHED_CONTRACT` lands on it, and the approve dialog will not close until the human acknowledges it. A weakened test command is the obvious cheat, and it deserves a human look, not a silent pass. A generated `.envrc` is excluded from the diff, so only a committed one can raise the flag.
- `.hulahoop/junit.xml` is the machine-readable verdict. Hulahoop parses test names and outcomes, so the red proof is "non-zero exit and the new tests are among the failures" and the green proof is "exit 0 and the authored tests are present and passing". The contract asks repos to ignore `.hulahoop/`.
- `just test` always runs the whole suite. Per-test selection stays out of the contract, which keeps the red proof valid across renames and framework filters.

A hard contract buys three things: the red/green proof is reproducible, agents and Hulahoop share one execution path, and there are no per-repo command settings. The cost is three machine prerequisites and no support for repos outside nix; the project screen says so in one sentence.

---

## 8. Agent adapter

```text
pi -p --mode json --model <model>
claude -p --output-format stream-json --model <model>
```

Both CLIs are run non-interactively, so the runner passes the flags that keep them from blocking on prompts: `pi -p --approve --tools …` and `claude -p --permission-mode …`. Each adapter declares its tool allowlist in config; the default excludes `git push`.

Hulahoop 0 ships three role-scoped agents:

| Role | Adapter | Input | Output |
| --- | --- | --- | --- |
| Test Author | Pi | issue title, body, criteria, base revision; no implementation | test-only diff |
| Implementer | Pi, separate model | frozen suite, criteria, prior review comments | implementation diff |
| Reviewer | Claude Code | issue, suite diff, implementation diff, both run logs | `approved` or `changes_requested` |

Different adapters and different roles make handoff bugs visible during dogfooding. Claude Code uses the machine’s existing Claude subscription or API credentials.

The adapter interface:

```rust
trait AgentAdapter {
    fn command(&self, run: &Run, checkout: &Path) -> Command;
    fn parse_event(&self, line: &str) -> Option<RunEvent>;
    fn cancel(&self, child: &mut Child) -> Result<()>;
}
```

### Preflight per adapter

Project setup probes each configured agent once, inside the repo's dev shell, and caches the result for a day:

| Check | Failure message |
| --- | --- |
| `direnv exec . pi --version` | "pi is not installed in this repo's dev shell" |
| `direnv exec . pi -p "ok" --model <model>` | "pi cannot reach <model> with the configured credentials" |
| `direnv exec . claude --version` | "Claude Code is not installed in this repo's dev shell" |
| `direnv exec . claude -p "ok" --model <model>` | "Claude Code is not logged in; run `claude` once in a terminal" |

The probe runs the way the run will, so an adapter that only exists inside the flake passes and one that only exists on the host PATH fails honestly.

A probe needs a working copy, and at `addRepo` time there is none, so the server creates a preflight checkout (`owner_kind = preflight`) under `repo_locks`, generates `.envrc` there, and points `HOME` at a scratch directory. The user's own working tree is never written to and its `.envrc` is never executed: a committed `.envrc` is still arbitrary shell, and v0 runs it on the user's machine only inside a checkout Hulahoop owns.

The outcome lands in `agent_preflight` keyed by `(agent_id, repo_id)`, because the probe answers a question about one adapter in one dev shell. A single row on `agents` would let repo B's toolchain overwrite repo A's verdict. The project screen reads those rows, and a failed probe blocks role assignment for that repo with the message above, so the first run does not die on a missing credential.

### Stacked runs

A `changes_requested` verdict moves the issue to `in_progress`, and the human starts the stacked run: base is the `issue/<id>/head/<attempt>` bookmark under review, `parent_run_id` points at that attempt, and the review body goes into the prompt. The frozen suite still comes from the `tests/<issue>` bookmark, and the diff under review is the accumulated diff against that base. The run detail screen shows the chain, so "runs per done issue under 3" counts attempts on the same change.

The runner sets:

```text
HULAHOOP_ISSUE_ID
HULAHOOP_RUN_ID
HULAHOOP_ROLE
HULAHOOP_CHECKOUT
HULAHOOP_BASE_BOOKMARK
HULAHOOP_TESTS_BOOKMARK
VCS=jj
NO_COLOR=1
```

The role prompts are files in the Hulahoop repo, embedded in the binary at build time:

```text
prompts/test_author.md
prompts/implementer.md
prompts/reviewer.md
```

Every project runs the same embedded prompt set. Per-project prompt overrides are out of scope in v0, so a prompt change is a Hulahoop release and never a repo edit. Each run records `runs.prompt_revision` as `<app version>+<sha256 of the embedded prompt set>`, the same in every checkout; a run's outcome then attributes to the exact prompt text that produced it, and the desktop shows the revision beside the run.

The Test Author reads the issue title, body, and acceptance criteria, plus repository instructions. It writes tests against the acceptance criteria on top of the base revision, touches nothing else, and leaves them failing. The Implementer reads the same issue, the frozen suite, and any prior review comments; it writes implementation code, keeps the diff focused, leaves the suite files alone, and reports evidence. The Reviewer reads the issue, the suite, the diff, and both run logs; it checks correctness, test integrity, and whether the acceptance criteria are actually met.

### The gate

1. The Test Author commits tests only, runs `just test` in its checkout, and reports the failure itself.
2. Hulahoop replays `just test` at the base revision in a clean checkout with the authored test files restored from `tests/<issue>/cand/<run-id>`. Every authored test must appear as a failure in `junit.xml`; that red log is the suite's evidence and is stored as a `replays` row of kind `base`, together with the `tests_bookmark` it applied.
3. `freezeSuite` sets the `tests/<issue>` bookmark and `tests_revision`, and moves the issue to `tests_ready`.
4. The Implementer branches from the `tests/<issue>` bookmark; the runner rejects a diff that touches any path in `issues.frozen_paths` and flags one that changes `flake.nix`, `flake.lock`, `justfile`, or a committed `.envrc`.
5. The Reviewer replays the suite at the base revision (red) and at the head revision (green), compares both `replays` rows, then reads both logs.

A suite that passes at the base revision proves nothing and is rejected. A suite that stays red at the head revision is a failed run. Both verdicts come from the JUnit reports rather than the agent's own summary.

---

## 9. Desktop UI

Four screens are enough for Hulahoop 0:

1. **Workspace and project**
   - workspace
   - project
   - repo path
   - base revision
   - test author, implementer, and reviewer agents, each with its preflight result
   - “Open project”

2. **Issues**
   - fixed columns: `backlog`, `ready`, `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging`, `done`
   - `blocked` and `cancelled` issues leave the columns and appear in a filter beside the board; `blocked` shows the pinned question, `cancelled` shows `runs.error`
   - `ready` means criteria approved; `tests_ready` means the suite is frozen and red at base
   - create issue
   - priority: low / medium / high
   - local notification when an issue enters `in_review`, `blocked`, or `merging`; an approved verdict in `in_review` notifies again, because that is when the human's click is the only thing left

3. **Issue detail**
   - title, body, acceptance criteria
   - freeze suite / thaw suite, the thaw asking for a reason
   - suite status: red at base, green at head
   - run / cancel per role
   - live log
   - diff
   - approve / request changes; approve is disabled until the Reviewer's verdict is `approved` and every flag is acknowledged
   - comments
   - `blocked` badge with the agent's open question pinned at the top, and a Resume box that takes the answer, writes it as a comment, and returns the issue to the status it left

4. **Run detail**
   - status, duration, exit code
   - checkout path
   - log
   - artifacts

No settings beyond the workspace/project screen. No theme picker. No dashboard.

---

## 10. GraphQL sketch

```graphql
type Query {
  workspaces: [Workspace!]!
  projects(workspaceId: ID!): [Project!]!
  repos(projectId: ID!): [Repo!]!
  agents(workspaceId: ID!): [Agent!]!
  issues(projectId: ID!, status: IssueStatus): [Issue!]!
  issue(id: ID!): Issue
  preflight(projectId: ID!): [AgentPreflight!]!
  runs(issueId: ID!): [Run!]!
  run(id: ID!): Run
  replays(issueId: ID!): [Replay!]!
}

enum IssueStatus {
  BACKLOG READY TESTS_IN_PROGRESS TESTS_READY IN_PROGRESS IN_REVIEW
  MERGING BLOCKED DONE CANCELLED
}

enum AgentRole { TEST_AUTHOR IMPLEMENTER REVIEWER }

type Mutation {
  createProject(input: CreateProjectInput!): Project!
  addRepo(input: AddRepoInput!): Repo!
  updateRepo(input: UpdateRepoInput!): Repo!
  createAgent(input: CreateAgentInput!): Agent!
  updateAgent(input: UpdateAgentInput!): Agent!
  setProjectAgents(input: SetProjectAgentsInput!): Project!
  createIssue(input: CreateIssueInput!): Issue!
  updateIssue(input: UpdateIssueInput!): Issue!
  approveCriteria(issueId: ID!): Issue!
  startRun(issueId: ID!, agentId: ID!, role: AgentRole!, parentRunId: ID): Run!
                        parentRunId stacks a rework run on the head under review
  cancelRun(runId: ID!): Run!         stops the process, the issue keeps its status
  abandonIssue(issueId: ID!, reason: String!): Issue!
                        the only path to `cancelled`
  freezeSuite(issueId: ID!): Issue!
  unfreezeSuite(issueId: ID!, reason: String!): Issue!
  approveIssue(
    issueId: ID!
    runId: ID!
    reviewedHead: String!
    diffSha: String!
    acknowledgeFlags: [RunFlag!]!
  ): Issue!
                        fails unless runId is issues.current_run_id, that run
                        carries an APPROVED review, reviewedHead and diffSha
                        match that run, and every flag on it is listed in
                        acknowledgeFlags; an empty list passes only for a run
                        with no flags
  recordMerge(issueId: ID!, mergeRevision: String!): Issue!
                        closes the issue once the landing replay is green
  resumeIssue(issueId: ID!, answer: String!): Issue!
                        writes the answer as a comment, clears pinned_comment_id,
                        returns the issue to status_before_blocked
  clearRunCap(issueId: ID!): Issue!     human override after 12 runs
  requestChanges(issueId: ID!, body: String!): Issue!
  addComment(issueId: ID!, body: String!): Comment!
}

type Subscription {
  issueChanged(projectId: ID!): Issue!
  replayChanged(issueId: ID!): Replay!
  runChanged(issueId: ID!): Run!
}
```

Objects, so every field a screen reads exists here:

```graphql
type Issue {
  id: ID!
  projectId: ID!
  repoId: ID!
  title: String!
  body: String!
  criteria: [Criterion!]!
  status: IssueStatus!
  priority: Priority!
  statusBeforeBlocked: IssueStatus
  pinnedComment: Comment
  attempt: Int!
  suiteSeq: Int!
  runCount: Int!
  runCapClearedAt: DateTime
  testsBookmark: String
  testsRevision: String
  frozenPaths: [String!]!
  testsFrozenAt: DateTime
  mergeRevision: String
  currentRun: Run        # the run `current_run_id` names; the run the approve
                         # dialog reviews and the only one approveIssue accepts
  runs: [Run!]!
  replays: [Replay!]!
  comments: [Comment!]!
  approvals: [Approval!]!
}

type Criterion { position: Int!, text: String!, testName: String }

type Run {
  id: ID!
  issueId: ID!
  agent: Agent!
  role: AgentRole!
  status: RunStatus!
  baseRevision: String!
  headBookmark: String
  headRevision: String
  parentRunId: ID
  attempt: Int!
  promptRevision: String!
  sandbox: String!
  flags: [RunFlag!]!      # empty list when the run is unflagged
  diffSha: String
  diff: String
  durationMs: Int
  exitCode: Int
  error: String
  checkoutPath: String
  artifactDir: String!
  log: String
}

# one suite execution Hulahoop ran itself, and the report the gate read
type Replay {
  id: ID!
  issueId: ID!
  runId: ID
  approvalId: ID
  kind: ReplayKind!
  attempt: Int!
  revision: String!
  testsBookmark: String
  status: ReplayStatus!
  ancestryOk: Boolean
  junitPath: String!
  logPath: String!
  createdAt: DateTime!
  finishedAt: DateTime
}

type Review { id: ID!, runId: ID!, verdict: ReviewVerdict!, body: String!, createdAt: DateTime! }
type Comment { id: ID!, authorType: AuthorType!, authorId: ID!, body: String!, createdAt: DateTime! }
type Approval { id: ID!, runId: ID!, headRevision: String!, diffSha: String!, landingBookmark: String, acknowledgedFlags: [RunFlag!]!, approvedBy: ID!, voidedAt: DateTime, createdAt: DateTime! }
type Repo { id: ID!, projectId: ID!, name: String!, path: String!, vcs: Vcs!, baseRevision: String!, contractVersion: String!, fetchedAt: DateTime }
type Agent { id: ID!, name: String!, adapter: Adapter!, model: String! }
type AgentPreflight { agentId: ID!, repoId: ID!, ok: Boolean!, detail: String, checkedAt: DateTime! }
type Project { id: ID!, workspaceId: ID!, name: String!, repos: [Repo!]!, agents: [ProjectAgent!]!, preflight: [AgentPreflight!]! }
type ProjectAgent { role: AgentRole!, agent: Agent! }
type Workspace { id: ID!, name: String!, slug: String!, projects: [Project!]!, users: [User!]! }
type User { id: ID!, name: String!, email: String! }

enum RunStatus { QUEUED RUNNING SUCCEEDED FAILED CANCELLED TIMED_OUT }
enum ReplayStatus { QUEUED RUNNING SUCCEEDED FAILED TIMED_OUT }
enum RunFlag { TOUCHED_CONTRACT LARGE_DIFF ARTIFACT_LIMIT }
enum ReplayKind { BASE HEAD LANDING }
enum ReviewVerdict { APPROVED CHANGES_REQUESTED }
enum AuthorType { HUMAN AGENT }
enum Priority { LOW MEDIUM HIGH }
enum Vcs { JJ GIT }
enum Adapter { PI CLAUDE_CODE }
```

Run logs use a separate WebSocket endpoint:

```text
GET /ws/runs/:runId
```

---

## 11. Dogfood plan

### Bootstrap paradox

Hulahoop cannot manage its own creation before it exists. Resolve it once:

1. Build Hulahoop 0 with the current tools.
2. Seed the first project: `hulahoop`.
3. Create the first issues in Hulahoop.
4. Run the first Hulahoop issue with Hulahoop.
5. From that point, every new feature, bug, and refactor is a Hulahoop issue.

### Dogfood rule

> If Hulahoop can manage it, it goes in Hulahoop.

Manual code changes are allowed only for bootstrap blockers. Every manual change becomes an issue afterward.

### First dogfood issues

Every one of these is a feature Hulahoop does not ship in M0, so each one exercises the loop that already exists.

- Add a Linux desktop build to the release pipeline.
- Add the QA role.
- Add a third agent adapter.
- Add a second project and a workspace switcher.
- Add Postgres mode.
- Add run export as a shareable evidence bundle.

The suite loop is not on this list: the Test Author, the freeze gate, and the base-revision replay ship in M0, so every dogfood run exercises the loop.

### Success metrics

- Hulahoop manages at least 80% of its own issues by week 4.
- Zero manual database edits after bootstrap.
- Runs per done issue under 6, which leaves room for one authoring, up to two implementation attempts, and two reviews.
- Median issue cycle under 1 day.
- No lost runs after server restart.
- Checkout cleanup succeeds on cancel, failure, and crash.
- Human interventions per issue under 2.

---

## 12. Milestones

### M0 — Walking skeleton (1–2 weeks)

- Rust workspace: `server`, `runner`, `cli`.
- SQLite schema and migrations for users, workspaces, workspace_members, projects, repos, agents, agent_preflight, project_agents, issues, criteria, checkouts, runs, replays, reviews, comments, approvals, repo_locks, run_events.
- GraphQL queries/mutations, including `recordMerge`, `resumeIssue`, and `clearRunCap`.
- Tauri shell.
- Workspace/project/repo/agent setup with preflight.
- Issue list and detail, including comments and the blocked badge with its pinned question.
- Run detail with status, duration, exit code, checkout path, log, and artifacts.
- Create/edit issue.
- Criteria editor: one line per criterion, numbered, and the `ac<position>` naming rule applied to test names.
- `flake.nix`, `.envrc`, and `justfile` for the Hulahoop repo itself, using the contract names: `setup`, `test`, `fmt-check`, `lint`, `ci`.
- `.github/workflows/ci.yml` running `nix develop --command just ci` on `ubuntu-latest` and `macos-14`.
- Repo contract 26.11: `flake.nix` and `justfile` preflight, generated `.envrc`, `direnv exec` entry, JUnit parsing.
- Suite freeze gate: base-revision red replay stored as a `replays` row, head-revision green replay at implementer completion, and the reviewer's `approved` verdict before an approval is accepted.
- Pi Test Author, Pi Implementer, Claude Code Reviewer, with three role prompts and adapter preflight.
- Fake adapter, fixture repo, and the end-to-end loop test.
- Run environment allowlist with a scratch `HOME`.
- jj checkout create/diff/cleanup. The git worktree provider ships in M1, so M0 supports jj repos only and refuses a git repo with a one-line reason.
- Run log stream.
- Local notifications for `in_review`, `blocked`, and `merging`.
- Approve / request changes, with the approval bound to the reviewed head and diff.
- Diff view, a text diff of the run against its base.
- Cancel with process-group kill.
- Landing replay on the current base, writing `landing/<issue>/<attempt>`.
- Thaw with a reason and abandonment, since both are on the detail screen.
- Restart recovery: `runs.runner_id` and `runs.claimed_at` decide which in-flight runs are orphaned.

**Exit:** one Hulahoop issue whose suite was authored by the Test Author, failed at the base revision, went green at the head revision, took an `approved` verdict from the Reviewer, and was approved by a human and merged through Hulahoop with a green landing replay. CI is green on both runners.

### M1 — Self-hosting (1 week)

- `bwrap` and `sandbox-exec` profiles for enforced confinement.
- `.github/workflows/release.yml` and `just bundle <target>`, producing unsigned `.dmg` artifacts for both macOS architectures.
- First dogfood issues completed.

**Exit:** the team uses Hulahoop for Hulahoop's backlog, and a `v0.0.1` tag publishes two unsigned `.dmg` builds.

### M2 — TDD roles (2 weeks)

- QA role.
- Additional agent adapters.
- Suite coverage checks: spot mutations of the implementation must turn the suite red.
- Review gate before QA.
- Evidence checklist.

### M3 — Remote mode (later)

- Postgres.
- Auth and users.
- Remote runner registration.
- Shared projects.
- GitHub/GitLab integration.
- Windows desktop builds, with a Job Object path for process-tree kill.

---

## 13. Open source and delivery

Hulahoop is MIT-licensed and local-first. The public repo is also the build and release path, and going public is what makes that path free: standard GitHub-hosted runners, macOS included, bill nothing on public repos. Microsoft's hosted macOS minutes are the expensive line item for a private repo — roughly ten times the Linux rate, 2000 included minutes a month on the free plan — so a public repo buys the whole release pipeline for the cost of a license file.

### Repository

- License: MIT. Apache-2.0 is the alternative if patent language matters more than simplicity; dual MIT/Apache-2.0 is the Rust-ecosystem default if the team wants both.
- `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, and issue templates.
- No API keys or signing secrets in the repo. Local mode uses a per-launch server token.
- Going public also publishes the dogfood backlog, since Hulahoop's own issues live in that repo. Dogfood issues stay phrased as product work, and anything sensitive moves to a private tracker.
- GitHub-hosted public-repo runners carry CI and unsigned releases at no cost; concurrency is the real ceiling (five concurrent macOS jobs), so the release job triggers on tags only.
- Check the Hulahoop name before launch: “Hula Hoop” is a Wham-O trademark in toys, and software is a different class. Get a short trademark review before spending on branding.

### CI

One workflow, one command, because the flake already carries every system dependency. Tauri needs `webkit2gtk`, `libsoup`, and `glib` on Linux; the dev shell provides them, so no distro package step exists and the workflow never drifts from a developer's machine.

```yaml
# .github/workflows/ci.yml
on: [push, pull_request]
jobs:
  ci:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-14]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: DeterminateSystems/magic-nix-cache-action@main
      - run: nix develop --command just ci
      - if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: junit-${{ matrix.os }}
          path: .hulahoop/junit.xml
```

CI calls `nix develop` rather than `direnv exec`, since a hosted runner has no direnv and nothing to allow. `magic-nix-cache` keeps the store warm between runs, so a no-op CI pass re-evaluates flake metadata and reuses every derivation. Determinate Systems has narrowed `magic-nix-cache` for public repos, so `cachix/cachix-action` with a free public cache is the drop-in fallback: one action in, one action out, same workflow shape.

### Release

Tag push, macOS-only, two architectures, artifacts uploaded per leg and published once:

```yaml
# .github/workflows/release.yml
on:
  push:
    tags: ['v*']
permissions:
  contents: write
jobs:
  macos:
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: macos-14      # aarch64-apple-darwin
            target: aarch64-apple-darwin
    runs-on: ${{ matrix.runner }}
    steps:
      - uses: actions/checkout@v4
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: DeterminateSystems/magic-nix-cache-action@main
      - run: nix develop --command just bundle ${{ matrix.target }}
      - run: codesign --verify --deep --strict target/${{ matrix.target }}/release/bundle/macos/Hulahoop.app
      - uses: actions/upload-artifact@v4
        with:
          name: dmg-${{ matrix.target }}
          path: target/${{ matrix.target }}/release/bundle/dmg/*.dmg
  publish:
    needs: macos
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          pattern: dmg-*
          merge-multiple: true
          path: dist
      - run: gh release create ${{ github.ref_name }} --generate-notes dist/*.dmg
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

The split matters: two matrix legs cannot both create the same release, so they build and upload, and one `publish` job creates it once after both succeed. A single failed leg leaves the tag without a release, which is the correct outcome for a tag.

The leg builds its own architecture natively, so no cross toolchain is involved. GitHub retired its hosted x86_64 macOS image, so an Intel build needs a larger runner or a local `just bundle x86_64-apple-darwin`. Linux artifacts (`AppImage`, `.deb`) join the same release from an `ubuntu-latest` job once the desktop app runs there, the M1 milestone after the macOS path works.

### Unsigned, and honest about it

An unsigned `.dmg` triggers Gatekeeper: macOS says the app is damaged or from an unidentified developer. Two things keep that tolerable:

- Ad-hoc signing (`codesign --force --deep --sign -`) is mandatory on Apple Silicon, since arm64 refuses to execute an unsigned binary at all. Tauri does this by default; the release step asserts it ran.
- The release notes and `README` give the one-line fix: right-click → Open, or `xattr -dr com.apple.quarantine /Applications/Hulahoop.app`.

The upgrade path is a paid Apple Developer account ($99/year) for Developer ID signing plus notarization, wired in as repository secrets. Nothing else in the pipeline changes when that happens: same job, two extra steps.

---

## 14. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Bootstrap takes longer than expected | Keep M0 to one project, one repo, two adapters, and three role prompts; the diff view is a text diff by choice, so that cut is already made |
| `nix`, `direnv`, or `nix-direnv` missing | Preflight checks all three with an install link; the desktop refuses to add the project without them, and prints the one-line reason |
| direnv cache invalidated by a `flake.lock` change | Re-evaluation once per repo per lock bump; `runs.toolchain_lock` makes the bump visible in the run list |
| Store and cache growth | Pinned flake inputs, one shared store per machine, documented `nix-collect-garbage -d`; checkouts cleaned per run |
| `.envrc` is executable code on shell entry | Runner generates it when absent, gates a committed `.envrc` like `justfile`, and allows per checkout path |
| jj op log is shared by every workspace | The agent's `jj new`/`describe`/`bookmark` write the same op log the human's workspace uses; concurrent ops surface as conflicts in whichever side loses the race. Retry on concurrent-op errors, keep the runner out of the human's workspace, and let `one_active_run_per_repo` enforce one agent per repo |
| Test-fitting: an Implementer writes tests that pass by construction | The Test Author never sees the implementation; the suite must fail at base; the runner blocks edits to frozen test files |
| Agent permissions: a non-interactive agent can run anything as the user | Per-adapter tool allowlist, no push in v0, documented flags (`pi -p --approve --tools …`, `claude -p --permission-mode …`) |
| Agent quality | Narrow prompts, acceptance criteria, small issues |
| Orphan processes | Process group kill, heartbeat timeout, startup cleanup |
| Disk growth | Per-run checkout cleanup, target-dir cleanup |
| GraphQL N+1 | DataLoader from the first nested field |
| Unsigned macOS builds | Ad-hoc signed, install notes with right-click-open and `xattr -dr com.apple.quarantine`; Developer ID signing and notarization arrive with a $99/year account and two extra workflow steps |
| Desktop packaging | Tauri builds are small, but WebView differences need testing on macOS and Linux; Windows arrives in M3 with a Job Object path for process-tree kill |
| Open-source support load | Keep adapters optional and document the local-only path |
| v0 has no filesystem confinement | An agent can read `$DATA`, including the connection file and the SQLite database, so a hostile run could claim work or write an approval; the token is only kept out of the environment. Accepted until M1 `bwrap`/`sandbox-exec` profiles land, and the project screen says so in one line |
| Scope creep | The out-of-scope list is a contract |

---

## 15. Open questions

1. Suite scope: criteria-only acceptance tests, or also regression tests around the touched code? Proposal: criteria-only in v0.
2. Thaw policy: a thaw happens only from `tests_ready`, so no run is in flight. Earlier attempts stay as history and `current_run_id` is cleared, so a stale head cannot be approved; whether the old attempts belong in the review screen is still open.
3. Sandbox depth: `bwrap` and `sandbox-exec` profiles land in M1. Does the Reviewer run confined too, given it only reads? Proposal: yes, same profile with a read-only checkout mount.
4. Remote runner protocol: reuse the internal HTTP/WS API or add a queue? Proposal: reuse it.
5. jj colocated and pure repos: support both, but test pure jj first.
6. Contract evolution: does M2 add a second contract (devcontainer, plain `justfile` without nix) or stay nix-only? `contract_version` makes either additive.
7. Workspace model: one local workspace with many projects, or multiple workspaces in the picker? Proposal: seed one workspace, but keep the picker and schema multi-workspace from v0.

---

## 16. Test strategy

Hulahoop's own repo follows the contract it ships: `just setup`, `just test`, `just ci`, with the suite writing JUnit to `.hulahoop/junit.xml`.

- **Fake adapter.** `FakeAgent` implements `AgentAdapter` and replays scripted JSONL from `fixtures/runs/*.jsonl`, so log streaming, cancel, timeout, crash, and both replays are testable without a model.
- **Fixture repo.** `fixtures/repo/` is a minimal nix+just repo whose suite is red against a known `tests_revision`, used by end-to-end tests for checkout creation, suite freeze, and replay.
- **State machine properties.** The transition table in §5 is the input: every state has a documented exit, no transition skips the gate, `cancelled` is reachable from every active state, and `done` is unreachable except through `recordMerge` behind a green landing replay.
- **Bookmark handles.** A test rewrites a commit in the run's workspace after the suite is frozen, then asserts the replay still resolves `tests/<issue>` and `issue/<id>/head/<attempt>`. That is the jj rewrite case from §6.
- **Gate cases.** A suite that passes at base is rejected; a suite that stays red at head fails the run; red-at-base plus green-at-head passes. All three run against the fixture repo.
- **Criteria matching.** Ten criteria, one of them named `ac10`, and a suite missing the test for criterion 1: the boundary match must reject the suite, while a substring match would accept it.
- **Transition precedence.** A run that ends with a question goes to `blocked` and never to `in_review`; a base replay landing while the issue is `blocked` leaves it blocked.
- **Replay outcomes.** A timed-out base replay is retried, a timed-out landing replay returns the issue to `in_progress`, and a green landing replay from an earlier attempt cannot satisfy `recordMerge` for the current one.
- **One end-to-end test** drives issue → suite → implementation → review → approve → landing replay against the fake adapter, and serves as the M0 exit criterion.

`just ci` runs `fmt-check`, `lint`, `just test`, and the desktop typecheck.

---

## 17. Definition of done

An issue is done when:

- acceptance criteria are met
- the suite failed at the base revision and passes at the head revision
- frozen test files are unchanged by the implementation
- the Reviewer's verdict on that run was `approved`
- the approval is bound to the run, head, and diff that were merged, and `current_run_id` was still that run
- the landing replay verified `landing/<issue>/<attempt>` and the merge contains it
- the human reviewed the diff and acknowledged every flag on the run
- the merge was replayed green against the current base
- the change is merged, with the merge commit recorded
- the run log and the JUnit reports are linked from the issue
- the checkout is cleaned up

If any of those are missing, the issue is not done.

---

## 18. What is built today

The loop above is implemented, not only specified. This section is the honest
difference between the spec and the code, and how to run it.

### Layout

| Path | What it is |
| --- | --- |
| `crates/core` | The state machine, the repo contract, the JUnit and diff rules, the run spec. No I/O, no framework. |
| `crates/server` | SQLite, the transition engine, the GraphQL API, and the internal API the runner talks to. The only writer of state and artifacts. |
| `crates/runner` | One loop: claim work, create a jj workspace, run `just setup`, spawn the adapter in its own process group, stream events, post the completion, run replays and jobs. |
| `crates/cli` | `hulahoop serve`, `hulahoop runner`, `hulahoop seed <repo>`, `hulahoop doctor`. |
| `apps/desktop` | Tauri 2 + React: board, issue detail with criteria and gates, approval dialog, settings, run log. Owns the server and runner processes. |
| `fixtures/repo` | A contract-compliant repository with `flake.nix`, `.envrc`, `justfile` and a `calc.py` the loop can extend. |
| `tests/e2e/loop.sh` | The whole loop, driven over GraphQL, against a throwaway copy of the fixture. |

### Running it

```sh
direnv allow            # or: nix develop
just setup              # the only recipe that needs the network
just test               # nextest + the JUnit report the gate reads
just lint               # clippy, warnings are errors
just ci                 # what CI runs
just dev                # server + runner + desktop against .hulahoop-dev
just seed /path/to/repo # create a project, a repo, three agents, three role bindings
tests/e2e/loop.sh       # the loop with the fake adapter, no model calls, no network
HULAHOOP_E2E_ADAPTER=real tests/e2e/loop.sh   # the same loop with real agents
```

`HULAHOOP_E2E_ADAPTER=real` uses `pi` for the Test Author and the Implementer. The
Reviewer uses `pi` on a machine without a Claude Code login and `claude_code` on one
with it (`HULAHOOP_REVIEWER_ADAPTER=claude_code`); the two adapters are the same code
path, only the argv differs.

### Where the implementation differs from the spec

The spec is the contract; these are the deliberate deviations, each because the code
found the spec wrong or incomplete. They are worth folding back into the sections
above when the loop has run for a while.

1. **`sqlx` with runtime-checked SQL instead of SeaORM.** The transitions are one
   statement each, plus a guard; an ORM's identity map and lazy relations earn
   nothing here, and `sqlx::migrate!` gives the migration story the spec asks for.
2. **No GraphQL subscriptions.** The desktop polls `issues` every 2.5s and the run log
   over `run_events(runId, afterSeq)`. A subscription needs a second transport for one
   screen; polling is one line and is trivially correct after a reconnect.
3. **A `runner_jobs` table** for fetch, preflight, merge verification and bookmark
   moves, instead of overloading `replays`. A replay is a suite run; a job is a piece
   of VCS or process work. Both are claimed over the same internal API.
4. **The candidate bookmark is written by the server at authoring time**
   (`tests/<issue>/cand/<run-id>`), not by `freezeSuite`. The freeze gate then reads
   the base replay that already exists, which keeps "freeze is a decision over stored
   evidence" true rather than making freeze do work.
5. **The `Fake` adapter is a script** (`HULAHOOP_FAKE_SCRIPT`, default
   `fake-agent.sh`), invoked through the same argv/spawn/stream/parse path as `pi` and
   `claude`. The product code holds no test scenarios, and the fake proves the real
   plumbing.
6. **Preflight probes run inside the dev shell** (`direnv exec . pi --version`) and are
   keyed per `(agent, repo)`, because the runner starts agents under `direnv` too.
7. **The agent's scratch `HOME` is seeded** with the adapter's own credentials
   (`~/.pi/agent`, `~/.claude`, and `~/.config/direnv` so `nix-direnv` reuses the
   cached dev shell instead of re-evaluating the flake). The environment stays the
   boundary; the login stays usable.
8. **`runs.agent.jsonl` is written by the runner** while it streams, because the
   review screen needs the transcript and the server only sees events.

### What is not built yet

- Merging from inside Hulahoop: the human merges, then records it. `recordMerge`
  verifies ancestry and the green landing replay at the merge revision.
- `bwrap` and `sandbox-exec` profiles (M1). The v0 boundary is the environment
  allowlist plus a scratch `HOME`; `~/.ssh`, `~/.aws` and the rest are never passed.
- Linux desktop builds and Windows. The flake builds and tests on Linux, the bundle
  path targets macOS.
- The macOS artifact: `.dmg` bundling runs in GitHub Actions
  (`.github/workflows/release.yml`) on a `v*` tag, unsigned, with the quarantine
  instructions from [13. Open source and delivery](#13-open-source-and-delivery).
  The workflow ships Apple silicon only: GitHub's hosted x86_64 macOS image is
  retired, so an Intel build needs a larger runner or a local `just bundle
  x86_64-apple-darwin`.
