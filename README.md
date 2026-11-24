# Hulahoop

**The acceptance loop that keeps coding agents honest.**

Hulahoop is a desktop-first, local-first workspace for running autonomous coding agents through one opinionated loop:

> backlog → to do → an agent works → in review → the pull request merges → done

It is intentionally small. One workflow document per workspace, at most one repository per run, one active run per issue, one person who merges, almost no settings.

> **The shipped build runs a simpler loop than sections 2–17 describe.** Those
> sections specify the frozen-suite model: a separate Test Author, a suite proven
> red at the base revision, replays, and an approval bound to a diff hash. The
> product outgrew that. Today a task moves through six statuses
> (Backlog, To do, In progress, Blocked, In review, Done), the workflow lives in a
> `WORKFLOWS.md` document per workspace, an agent owns a status and is dispatched
> when work enters it, and the merged pull request is what closes a task. Section
> [19](#19-the-loop-as-it-ships) is the current model; the rest is the reasoning
> that led here, kept because it explains the constraints.

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
- Statuses name their agents in the workspace's `WORKFLOWS.md`, so one workflow can put a planner, a writer and a reviewer in the same status.
- Three agents: Pi Test Author, Pi Implementer on a different model, Claude Code Reviewer. The human approves and merges.
- One frozen test suite per issue, authored before any implementation and replayed at the base revision.
- One repo contract: `flake.nix` and `justfile` with `setup` and `test` recipes, plus the `.envrc` the runner generates. No per-repo command settings.
- A per-run sandbox: a scratch `HOME` and a clean environment in v0, extended to a read-only host outside the checkout in M1.
- Local notifications when an issue needs a human: `in_review`, `blocked`, and `merging`.
- Lumi, the workspace's assistant: a pill in the corner of every screen (⌘J). It answers
  with the harness and model picked in Settings, reads a snapshot of the board, and
  proposes changes as checkboxes that run only when the person presses Apply. A question
  travels through the runner like any job, so the server never talks to a model itself.
- SQLite database.
- One server that owns the work, on `127.0.0.1` by default and on any address with `--listen`. The desktop is a client: it points at a server, and the server's host runs the agents.

- Desktop app with:
  - project picker
  - issue list
  - issue detail
  - create/edit issue
  - acceptance criteria field
  - suite status: red at base, green at head
  - “Run” button that starts the agent the status names
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
- Cloud hosting, public ingress, hosted multi-tenant runners. Remote mode is local-first: you run the server yourself (see §18).
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

**`hulahoop serve`**
- Rust, Axum, `async-graphql`, SeaORM.
- Owns the state machine, database, approvals, run history, the log files, and the artifact directories.
- The only process that writes SQLite. The runner reports facts over HTTP, so there is one writer and no cross-process lock contest.
- Owns `repo_locks`. The runner asks for and releases a lock over the internal API, so every lock has one owner and one expiration.
- Binds `127.0.0.1` on a random port by default; `--listen <address>` opens it deliberately, which is what remote mode uses.
- Writes a connection file with `{ port, token, listen, url }` for the desktop app; `--print-connection` prints the same JSON for a remote operator.
- Exposes GraphQL for the desktop.
- Exposes a small internal API for the runner.

**`hulahoop runner`**
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
- Spawns sidecar `hulahoop serve` and `hulahoop runner` binaries in local mode.
- Opens the UI against the local server, or against any server whose address and token you paste into the connect screen (saved per machine).
- Polls `run_events` while a run is live, and re-reads issues every 2.5 s.

**`hulahoop` CLI**
- Bootstrap and debugging helper.
- `hulahoop server`, `hulahoop runner`, `hulahoop issue new`, `hulahoop suite replay <issue>`.
- Not the primary UI.

### Why desktop first

- Direct access to local repos, jj, Git credentials, and agent CLIs.
- No public ingress, TLS, or account system.
- One machine to debug.
- The server and runner stay headless, so the same binaries serve a laptop and a workstation: the desktop points at either one.

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
| Agents | Multiple, each bound to the statuses that name it |
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
  id, name, slug, lumi_harness, lumi_model, created_at
                        lumi_harness, lumi_model: what Lumi answers with;
                          null until somebody picks one in Settings

workspace_members
  workspace_id, user_id, role

projects
  id, workspace_id, name, created_at

repos                   a project can have more than one
  id, project_id, name, remote_url, vcs, base_bookmark, base_revision,
  contract, contract_version, fetched_at, created_at
                        remote_url: what the mirror is cloned from; there is no
                          local path, a repository is always Hulahoop's own clone
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

issues
  id, project_id, repo_id, title, body, status, priority,
  status_before_blocked, pinned_comment_id, run_cap_cleared_at,
  attempt, suite_seq, run_count, current_run_id, tests_revision,
  tests_bookmark, frozen_paths, tests_frozen_at,
  merge_revision, merged_at, created_at, updated_at,
  pr_number, pr_url, pr_branch
                        pr_number: written when the runner reports the pull
                          request it opened, so the issue links it and a merge
                          reads GitHub instead of falling back to the run's head
                        pr_branch: the branch the pull request carries
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

checkouts              unused since checkouts became one per task; a task's
                          checkout is found by its path (§6), not by a row

runs
  id, issue_id, repo_id, agent_id, attempt, base_bookmark, base_revision,
  head_revision, head_bookmark, parent_run_id, runner_id, claimed_at,
  last_heartbeat_at, pid, pgid, toolchain_lock, prompt_revision, sandbox,
  env_keys, artifact_dir, diff_sha, flags, status, started_at, finished_at,
  exit_code, error, tokens_in, tokens_out
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
                        tokens_in, tokens_out: what the adapter's stream reported,
                          one value per message at its last snapshot; 0 when the
                          stream carries no usage; read on every path that
                          finishes a run, including cancel and the dead-runner
                          reaper

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

lumi_messages         the workspace's conversation with Lumi, oldest first
  id, workspace_id, role, body, context, proposal, proposal_state, created_at
                        role: user | lumi; the user row lands at once, the lumi
                          row arrives when the runner's job completes
                        context: what the person was looking at when they asked
                        proposal: JSON {title, items:[{group,text,action,args}]},
                          the checkboxes; null when the answer is only words
                        proposal_state: open | applied | dismissed — Apply runs
                          the ticked items, and nothing changes before that

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

The project, milestone and artifact tables are described under
[Projects, milestones and artifacts](#projects-milestones-and-artifacts).

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
| `ready` | `startRun(agentId)` | criteria valid, the status names that agent, no run queued or running on the repo | `tests_in_progress` |
| `tests_in_progress` | `startRun(agentId)` | no run queued or running, reached by a thaw or a rejected freeze | `tests_in_progress` |
| `tests_in_progress` | authoring run completes | writes `tests/<issue>/cand/<run-id>` and queues the base replay | `tests_in_progress` |
| `tests_in_progress` | base replay completes | it ran the candidate suite against `repos.base_revision` | `tests_in_progress` |
| `tests_in_progress` | `freezeSuite(issueId)` | base replay present, every authored test fails in it, each criterion maps to exactly one test name, no run queued or running | `tests_ready` |
| `tests_in_progress` | `freezeSuite(issueId)` rejected | an authored test passes at base, or a criterion has no test | `tests_in_progress` |
| `tests_in_progress` | authoring run fails | `runs.error` records why, nothing is frozen yet | `ready` |
| `tests_ready` | `startRun(agentId)` | suite frozen, no run queued or running on the repo | `in_progress` |
| `tests_ready` | `unfreezeSuite(reason)` | human, archives `tests/<issue>` as `tests/<issue>/archive/<suite_seq>` and clears `current_run_id` | `tests_in_progress` |
| `in_progress` | `startRun(agentId, parentRunId)` | no run queued or running; the rework run branches from that run's head bookmark | `in_progress` |
| `in_progress` | implementer run completes | head replay green, frozen test files unchanged, sets `current_run_id` | `in_review` |
| `in_progress` | implementer run fails | `runs.error` records why; the issue stays put so the human can start another attempt | `in_progress` |
| `in_review` | `startRun(agentId)` | `current_run_id` set and that run has a head bookmark | `in_review` |
| `in_review` | reviewer verdict `changes_requested` | review row written, body quotes the failing tests | `in_progress` |
| `in_review` | reviewer verdict `approved` | review row written, `open_pr` queued for the newest run that published a head, the approval still needs a human | `in_review` |
| `in_review` | `requestChanges(body)` | human, comment recorded | `in_progress` |
| `in_review` | `approveIssue(runId, reviewedHead, diffSha, acknowledgeFlags)` | `runId` is `current_run_id`, that run has an `approved` review, head and diff match it, every flag acknowledged | `merging` |
| `merging` | landing replay turns red or times out | failing test names in the replay report, the approval is voided | `in_progress` |
| `merging` | `recordMerge(mergeRevision)` | a green landing replay for this approval exists, `ancestry_ok` is true | `done` |
| `blocked` | `resumeIssue(issueId, answer)` | writes the answer comment, puts the task back in `status_before_blocked` with a fresh round of turns, and starts the agent that status names; `pinned_comment_id` retires when the run carrying the question and the answer is built, `status_before_blocked` is one of `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | `status_before_blocked` |
| `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | agent ends its run with a question | comment recorded and pinned | `blocked` |
| `tests_in_progress`, `tests_ready`, `in_progress`, `in_review`, `merging` | `cancelRun(runId)` | process group killed; the issue keeps its status and loses its active run | unchanged |
| any state except `done` and `cancelled` | `abandonIssue(reason)` | human | `cancelled` |

`done` and `cancelled` are terminal. `done` is reachable only through `recordMerge` behind a green landing replay. `cancelled` means the human abandoned the issue, which is why it has its own mutation: cancelling a run stops the process and leaves the issue where it was, so a misclick costs one run and never the issue. `blocked` is not an active state for the purposes of this table: it holds no run, and every row that leaves it is a human action.

Two events can arrive for one run, so precedence is explicit: a run whose final message is a question fires the `blocked` row and does **not** fire the completion row, because "completes" in that table means a run that finished without a question. A base replay queued before a block still lands while the issue is `blocked` and leaves it `blocked`.

The Reviewer works while the issue sits in `in_review`, so its own row leaves the status alone until a verdict lands; an `approved` verdict still waits for the human, which is what the independence in §3 buys. An approving verdict opens the pull request at once, naming the newest turn that published a head, so the work sits on GitHub before the person reads the verdict; a reviewer's own turn publishes no head, and naming one asks the runner to push a branch that does not exist. An agent asks a question by ending its run with that question, so `blocked` never holds a live process: the run completes, `pinned_comment_id` points at the question, and the next run's prompt carries the human's answer. A question asked while the issue is already blocked leaves `status_before_blocked` alone, so `resumeIssue` cannot land back on `blocked`.

The rework loop is the same `startRun` reached from `in_progress` with `parentRunId`, which is why a rejected diff, a changes-requested verdict, and a red landing replay all converge on `in_progress` instead of forking into their own states.

Three rules live in the schema, so a race cannot violate them:

- `create unique index one_active_run on runs(issue_id) where status in ('queued','running')` gives one queued or running run per issue. A second `startRun` fails with "a run is already active".
- `create unique index one_active_run_per_repo on runs(repo_id) where status in ('queued','running')` gives one running agent per repo. Two agents in one repo would fight the same jj op log and the same `cargo`/`pnpm` cache, so the second run queues behind the index instead of behind a sentence in the risk table. Parallel work across projects still works; parallelism inside one repo arrives when replays get a build cache of their own.
- `startRun(issueId, agentId)` starts the agent the status names, and rejects one the workflow does not name for that status. A status that names `any` gets the agent whose last run is oldest, so a workflow that names nobody by hand still has a turn to serve.

### Defaults

Constants in code, not settings. The UI shows them where they bite:

| Limit | Value |
| --- | --- |
| runner heartbeat | 5s |
| runner declared dead after | 30s without a heartbeat |
| agent wall clock, `just setup` excluded | 30 min per run |
| `just setup` | 20 min |
| one `just test` replay | 15 min |
| runs per issue | 12 rows, which covers a work turn and its review across four attempts; the Run button disables until a human calls `clearRunCap` |
| GraphQL request | 30s |
| SQLite `busy_timeout` | 5s |
| log rotation | at 50 MB a run keeps its first and last 50 MB (`agent.jsonl`, `agent.1.jsonl`); metadata retained forever |
| diff size | 2000 changed lines, then the run is flagged for human review |
| run artifact directory | 500 MB, then the run fails with status `failed` and the `ARTIFACT_LIMIT` flag |
| connection file mode | `0600`, read by the desktop at launch |

A run that exceeds its time limit is killed by process group and recorded as `timed_out`. A missing heartbeat triggers the same kill path from the server side, so a crashed runner cannot leave agents running. The artifact limit is not a time limit: that run ends as `failed` with `ARTIFACT_LIMIT` on it, because its evidence is unusable.

---

## 6. Checkouts

A checkout is the working copy a task's turns run in: a jj workspace of a repository Hulahoop owns. It is separate from a Hulahoop workspace, which is the top-level container for projects.

All of it lives under the data directory, and nothing outside it is ever worked in:

```
$DATA/mirrors/<repo-id>                  a clone of the repository's url (jj git clone --colocate)
$DATA/checkouts/<project-id>/<issue-id>  the task's checkout, a workspace of that mirror
```

`$DATA` is the data root: `~/.local/share/hulahoop` by default, `~/Library/Application Support/hulahoop` on macOS, overridable with `HULAHOOP_DATA`. Every path in this document is relative to it.

Before migrations run, the server copies `hulahoop.sqlite` (and its `-wal`/`-shm` sidecars) to `$DATA/backups/hulahoop-<timestamp>.sqlite` and keeps the newest five. This exists because a migration that rebuilds a parent table — `drop table repos` to make `repos.path` nullable, for instance — performs an implicit delete while foreign keys are enforced, and every child row goes with it. A copy from the last start is the only thing that turns that from data loss into a slow start.

Rules:

- A repository is attached by its url (`https://`, `ssh://`, `git://`, `file://`, an absolute path, or `user@host:path`). The runner clones it into `mirrors/<repo-id>` the first time it is needed, into a temporary directory that is renamed into place, and fetches it before every run and job after that. One in-process lock per mirror keeps a clone, a fetch and a landing from interleaving. Changing a repository's url queues a `forget_mirror` job that deletes that mirror, so the next job clones the new url.
- One checkout per task, at `checkouts/<project-id>/<issue-id>`, reused by every turn of the task. A later turn moves it to its start revision with `jj new` and keeps ignored build output (`target/`, `node_modules`, `.venv`), so a rework starts warm; tracked files are restored to that revision. A directory that cannot be reused is replaced by a fresh `jj workspace add`.
- The runner derives the checkout path from the project and issue ids and refuses anything that is not exactly `checkouts/<uuid>/<uuid>` under the data directory, with no symlinked component, before it creates or deletes one.
- A `cleanup` job forgets the workspace and deletes the directory once the task is Done or Cancelled and no run of it is live. Reopening the task drops a cleanup that has not run yet; moving the task to another project cleans up the checkout it left behind.
- A fetch writes the base bookmark's revision to `repos.base_revision` with `repos.fetched_at`, and never moves it back to an ancestor of the recorded one. A merge without a pull request queues a `land` job: the runner moves the base bookmark forward to the merged head in the mirror and pushes it (`jj git push -b <base>`), and only then does the head become `repos.base_revision`. A landing that cannot move forward fails and says so on the task.
- `.hulahoop/` stays out of the diff through a fileset exclusion on every diff command (`~glob:.hulahoop/**`). Where a `.git` directory exists, the runner also writes that pattern into `.git/info/exclude` so `git status` in the checkout stays clean for the agent.
- The generated `.envrc` needs the same treatment, and only when the runner generated it. A committed `.envrc` is tracked, appears in the diff like any other file, and raises `TOUCHED_CONTRACT`.

The sections below on bookmarks and replays describe the original test-first design. Replays and the `replays` table are gone (migration `0024_repos_from_urls.sql`).

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

Hulahoop 0 ships three agents, and the default workflow names them by name rather than by kind:

| Agent | Adapter | Input | Output |
| --- | --- | --- | --- |
| Test Author | Pi | issue title, body, criteria, base revision; no implementation | test-only diff |
| Implementer | Pi, separate model | frozen suite, criteria, prior review comments | implementation diff |
| Reviewer | Claude Code | issue, suite diff, implementation diff, both run logs | `approved` or `changes_requested` |

Different adapters and different agents make handoff bugs visible during dogfooding. Claude Code uses the machine’s existing Claude subscription or API credentials.

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

The outcome lands in `agent_preflight` keyed by `(agent_id, repo_id)`, because the probe answers a question about one adapter in one dev shell. A single row on `agents` would let repo B's toolchain overwrite repo A's verdict. The project screen reads those rows, and a failed probe keeps that agent out of that
repo's runs with the message above, so the first run does not die on a missing credential.

### Stacked runs

A `changes_requested` verdict moves the issue to `in_progress`, and the human starts the stacked run: base is the `issue/<id>/head/<attempt>` bookmark under review, `parent_run_id` points at that attempt, and the review body goes into the prompt. The frozen suite still comes from the `tests/<issue>` bookmark, and the diff under review is the accumulated diff against that base. The run detail screen shows the chain, so "runs per done issue under 3" counts attempts on the same change.

The runner sets:

```text
HULAHOOP_ISSUE_ID
HULAHOOP_RUN_ID
HULAHOOP_AGENT
HULAHOOP_CHECKOUT
HULAHOOP_BASE_BOOKMARK
HULAHOOP_TESTS_BOOKMARK
VCS=jj
NO_COLOR=1
```

What an agent is told comes from three places, and Hulahoop hardcodes none of them:

- **The run contract**, one built-in skill (`crates/server/skills/run-contract.md`, embedded in the binary). It carries the mechanics the server has to read back: one turn, where the task is, the house rules, and the `HULAHOOP_VERDICT:` / `HULAHOOP_QUESTION:` line a turn ends with. It is attached to every agent, shown read-only in Skills, and is the one text a person cannot edit.
- **The agent's own instructions** (`agents.agents_md`), written by whoever best knows that agent. An architect that plans and a developer that implements differ here, and only here.
- **Its skills**: `SKILL.md` files with `name` and `description` front matter. The prompt lists each one by description, and the runner writes the bodies to `.hulahoop/skills/<name>/SKILL.md` in the checkout, which every repository ignores. The harness opens the one whose description fits the work; the bodies of the others never cost context.

Nothing about an agent's kind is stored, because nothing about it needs to be: what a turn *was* is read from what it did. A turn that changed the checkout carries a commit — the runner names `issue/<issue id>/head/<attempt>` at it, and that head is what a pull request and the next turn point at. A turn that changed nothing publishes nothing, so the workflow can put a planner, a validator and a rework in the same status without the server knowing which is which. A closing line of `HULAHOOP_VERDICT: approved` or `changes_requested` judges the work before it and is recorded as a review; the agent is what its `AGENTS.md` says it is. Each run records `runs.prompt_revision` as `<app version>+<sha256 of the agent's instructions>`, so an outcome attributes to the text that produced it, and the desktop shows the revision beside the run.

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
   - run / cancel per agent the status names
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
  lumiMessages(workspaceId: ID!): [LumiMessage!]!
}

enum IssueStatus {
  BACKLOG READY TESTS_IN_PROGRESS TESTS_READY IN_PROGRESS IN_REVIEW
  MERGING BLOCKED DONE CANCELLED
}

type Mutation {
  createProject(input: CreateProjectInput!): Project!
  addRepo(input: AddRepoInput!): Repo!
  updateRepo(input: UpdateRepoInput!): Repo!
  createAgent(input: CreateAgentInput!): Agent!
  updateAgent(input: UpdateAgentInput!): Agent!
  createIssue(input: CreateIssueInput!): Issue!
  updateIssue(input: UpdateIssueInput!): Issue!
  approveCriteria(issueId: ID!): Issue!
  startRun(issueId: ID!, agentId: ID!, parentRunId: ID): Run!
                        agentId must be one the status names, or `any`
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
                        writes the answer as a comment, returns the issue to
                        status_before_blocked, and starts the turn it names
  clearRunCap(issueId: ID!): Issue!     human override after 12 runs
  moveIssue(issueId: ID!, status: String!): Issue!
                        a person's move, allowed only where the current status's
                        `Next:` lines say the task may go; `Issue.moves` lists
                        them, which is what the drawer's Move panel shows
  requestChanges(issueId: ID!, body: String!): Issue!
  addComment(issueId: ID!, body: String!): Comment!

  setWorkspaceLumi(workspaceId: ID!, harness: String!, model: String!): Workspace!
  askLumi(workspaceId: ID!, text: String!, context: String): LumiMessage!
                        stores the question and queues a `lumi` job; the answer
                        arrives as its own message when the runner finishes
  applyLumiProposal(messageId: ID!, items: [Int!]!): LumiMessage!
                        runs the ticked items of an open proposal, in order,
                        stopping at the first failure with a count of what landed
  dismissLumiProposal(messageId: ID!): LumiMessage!
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
  tokensIn: Int!
  tokensOut: Int!
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
type Project { id: ID!, workspaceId: ID!, name: String!, repos: [Repo!]!, preflight: [AgentPreflight!]! }
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
- `.github/workflows/ci.yml` running `nix develop --command just ci` on `ubuntu-latest` and `macos-latest` (macOS 26 on arm64 today).
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

### M3 — Hosted mode (later)

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
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: nix-community/cache-nix-action@v7
        with:
          primary-key: nix-${{ runner.os }}-${{ hashFiles('flake.lock') }}
          restore-prefixes-first-match: nix-${{ runner.os }}-
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            apps/desktop/src-tauri -> target
      - run: nix develop --command just ci
      - if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: junit-${{ matrix.os }}
          path: .hulahoop/junit.xml
```

CI calls `nix develop` rather than `direnv exec`, since a hosted runner has no direnv and nothing to allow. Two caches keep a no-op pass cheap: `nix-community/cache-nix-action` restores `/nix/store` through the Actions cache, keyed on `flake.lock`, and `Swatinem/rust-cache` restores `target/` for both Cargo workspaces. `DeterminateSystems/magic-nix-cache` was the obvious pick, but it now requires FlakeHub registration, so the community action stands in; `cachix/cachix-action` with a free public cache is the other drop-in.

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
          - runner: macos-latest  # aarch64-apple-darwin
            target: aarch64-apple-darwin
    runs-on: ${{ matrix.runner }}
    steps:
      - uses: actions/checkout@v4
      - uses: DeterminateSystems/nix-installer-action@main
      - uses: nix-community/cache-nix-action@v7
        with:
          primary-key: nix-${{ runner.os }}-${{ hashFiles('flake.lock') }}
          restore-prefixes-first-match: nix-${{ runner.os }}-
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: |
            . -> target
            apps/desktop/src-tauri -> target
      - run: nix develop --command just bundle ${{ matrix.target }}
      - run: |
          APP=apps/desktop/src-tauri/target/${{ matrix.target }}/release/bundle/macos/Hulahoop.app
          for bin in "$APP"/Contents/MacOS/*; do codesign --verify --strict "$bin"; done
          codesign -dvv "$APP" 2>&1 | grep -q "Signature=adhoc"
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

- Ad-hoc signing (`codesign --force --deep --sign -`) is mandatory on Apple Silicon, since arm64 refuses to execute an unsigned binary at all. Tauri does this by default, `tauri.conf.json` pins it with `"signingIdentity": "-"`, and the release step asserts the result: it verifies `Contents/MacOS/Hulahoop` and checks that the signature reads `Signature=adhoc`. Verifying the whole bundle with `--deep --strict` instead reports "code has no resources but signature indicates they must be present" for an ad-hoc signed `.app` — a `codesign` quirk, not a bundling defect.
- The release notes and `README` give the one-line fix: right-click → Open, or `xattr -dr com.apple.quarantine /Applications/Hulahoop.app`.

The bundle is built inside the nix dev shell, whose linker resolves libraries through `/nix/store`. A Mach-O that records those paths launches only on a machine that has nix installed — dyld aborts with `Library not loaded: /nix/*/libiconv.2.dylib`, which is how the first shipped `.dmg` failed. `scripts/macos-fix-deps.sh` therefore runs on every macOS bundle: it rewrites each `/nix/store` dependency to the system library of the same name (`/usr/lib/libiconv.2.dylib`), carries anything without a system counterpart inside `Contents/Frameworks`, drops store rpaths, re-signs ad-hoc, and fails the build if a store path survives. The release job repeats the `otool -L` check and refuses to publish a dmg that still points into `/nix/store`.

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
| `crates/runner` | One loop: claim work, bring the task's checkout onto its start revision, run `just setup`, spawn the adapter in its own process group, stream events, post the completion, run jobs (fetch, land, cleanup, forget_mirror, preflight). |
| `crates/cli` | `hulahoop up`, `hulahoop serve`, `hulahoop runner`, `hulahoop seed <repo>`, `hulahoop doctor`. |
| `apps/desktop` | Tauri 2 + React: connect screen, board, issue detail with criteria and gates, approval dialog, settings, run log. Spawns a server and runner for local mode, and is a plain client of whatever host it is pointed at. |
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
just seed /path/to/repo # create a project, a repo, three agents, and the workflow that names them
hulahoop up             # server and runner on this host, one command (--listen, --port, --data-dir)
just dev 0.0.0.0 9000   # debug build of server + runner, on every interface, so a Mac can connect
just debug 192.168.1.19 9000   # why can't the app reach it: address, port, /health, token
just check-ui           # every document the UI sends, against a live schema
tests/e2e/loop.sh       # the loop with the fake adapter, no model calls, no network
HULAHOOP_E2E_ADAPTER=real tests/e2e/loop.sh   # the same loop with real agents
```

A build takes its version from the release tag, not from `tauri.conf.json`:
`scripts/release-version.sh` answers `0.0.12` for `v0.0.12` (from `HULAHOOP_VERSION`,
which the release workflow sets from `github.ref_name`, or from the tags at `HEAD`),
and `just bundle` passes it through `--config`, so the dmg, the bundle and the About box
all say the same number as the release. `tauri.conf.json` holds `0.0.0` as a placeholder
for a tree with no tags at all.

`just check-ui` exists because the desktop sends GraphQL as strings: no compiler sees
them, so a field the server stops exposing fails at the first request instead of at
build time. It posts every document from `apps/desktop/src` to a throwaway server and
names the file and the message when the schema has moved on.

`HULAHOOP_E2E_ADAPTER=real` uses `pi` for the Test Author and the Implementer. The
Reviewer uses `pi` on a machine without a Claude Code login and `claude_code` on one
with it (`HULAHOOP_REVIEWER_ADAPTER=claude_code`); the two adapters are the same code
path, only the argv differs.

The app needs macOS 14 (Sonoma) or newer. The Rust toolchain in the flake builds its
standard library with a 14.0 deployment target, so linking against 12.0 prints
`was built for newer macOS version (14.0) than being linked (12.0)` and would promise
APIs macOS 12 does not have; `tauri.conf.json` pins `minimumSystemVersion` to the same
value rather than shipping that mismatch.

### Running the loop on another machine

The desktop is a client in the strict sense: it holds no state. One machine does the
work — the one with `nix`, `direnv`, `jj`, and the agent CLIs — and it runs two
processes there. The server owns the database, the state machine, and the artifacts;
the runner owns the mirrors, the checkouts, and the agent processes. They stay separate
so a crashed agent cannot take the server with it, and so each run is killed by process
group. The laptop only needs the app.

One command starts both:

```sh
# on the workstation that holds the repo and the toolchain
hulahoop up --listen 0.0.0.0 --port 9000
#   listening listen=0.0.0.0 port=9000 url=http://192.168.1.19:9000
#   server and runner are up server=http://192.168.1.19:9000 …

jq -r .token ~/.local/share/hulahoop/connection.json   # the token, from another shell
```

One executable covers both halves: `up` runs `serve`, waits for the connection file,
then runs `runner` with `--server` and `--token`, and stops the other when either exits.
Running one half by hand (`hulahoop serve --listen 0.0.0.0`, `hulahoop runner --server
http://host:9000 --token <token>`) is what you do when they live on different machines.

Where the binaries come from:

- **In the app.** `Hulahoop.app/Contents/MacOS/hulahoop` is the same binary; the app
  runs it as `hulahoop serve` and `hulahoop runner` in local mode.
- **A release asset.** Each release publishes three tarballs, each holding one
  `hulahoop` binary: `hulahoop-aarch64-apple-darwin.tar.gz`,
  `hulahoop-x86_64-unknown-linux-gnu.tar.gz`, and
  `hulahoop-aarch64-unknown-linux-gnu.tar.gz`. Unpack the one that matches the host and
  run `./hulahoop up`; no checkout, no nix. The Linux builds come from plain `cargo`
  on Ubuntu runners rather than the nix shell, because the nix linker records
  `/nix/store` paths for glibc — the same trap the macOS bundle had. The glibc floor
  is 2.35 on x86_64 (Ubuntu 22.04) and 2.39 on arm64 (the oldest hosted arm image).
  The desktop app ships for Apple silicon only; a desktop build for Linux is M3.
- **From source.** `nix develop --command cargo build --release -p hulahoop-cli`
  writes `target/release/hulahoop`.

Then paste `http://workstation:9000` and the token into the app's connect screen. The
app remembers the address; the token it does not, because a server mints a fresh one
on every start, so a restarted host means pasting again. "Use this machine" switches
back to the bundled local server. The runner takes `--server`/`--token`, or `HULAHOOP_SERVER` and
`HULAHOOP_TOKEN`; without them it reads the `connection.json` a local server wrote,
which is what `just dev` relies on. `tests/e2e/loop.sh` starts its runner the remote
way for exactly that reason, so the path is exercised on every CI run.

Two limits worth knowing. The token is one bearer secret over plain HTTP: keep the port
on a private network (Tailscale) or behind `ssh -L 9000:localhost:9000 workstation`.
Repositories are attached by url and cloned into the runner's data directory, so the
runner needs to reach that url; a local path only works when it exists on the runner's
host.

`--listen` defaults to `127.0.0.1`. With `--listen 0.0.0.0` the connection file
advertises the first outside address the host has, which is the address to give the
Mac; opening a port is a decision, never a default.

The app reaches that address over plain HTTP. Its content-security policy permits
`http://` and `ws://` to any host, and macOS asks for Local Network permission the
first time an app really opens a socket to the LAN — an entry can only appear after
that first attempt, so a request that never shows up means the app is running from a
translocated path such as `~/Downloads`, or a VPN is handling the traffic first.

### Connectors

The loop acts on your behalf in two different ways, and both are connectors: a GitHub
app pushes commits and opens pull requests as a bot user, and an agent CLI logs in as
your subscription.

| provider | kind | what is stored | what a run gets |
| --- | --- | --- | --- |
| `github` | `github_app` | app id + private key | `GITHUB_TOKEN`, the bot commit identity, the pull request |
| `claude_code` | `subscription` or `api_key` | `.claude/.credentials.json` or `ANTHROPIC_API_KEY` | a copy of that file in the run's scratch home, or the variable |
| `pi` | `subscription` or `api_key` | `.pi/agent/auth.json` or `PI_API_KEY` | the same |

Codex is not a harness of its own: pi runs those providers with its own sign-in, so the `codex` connector was removed rather than maintained twice.

Every payload is sealed with the vault key before it touches the database, field by
field: reading the app id never requires opening the private key. Settings shows one row
per provider with its state — *connected*, *stored, unusable*, *not stored* — and never
echoes a secret back. *Connected* means the stored secret can still act: a GitHub key
that does not parse is reported as exactly that, because a key that cannot sign is not
the app it claims to be.

A run gets the connector for **its own adapter**, in the spec, over the authenticated
internal channel. The desktop never sees a secret; the agent sees only what its own CLI
needs, in a scratch home it does not share with the next run.

The order is deliberate: a GitHub app first, because attribution and pull requests are
how the loop's work reaches a human for review; agent subscriptions second, because a
CLI that cannot log in produces nothing at all.

### Where a run works

Every run works inside the data directory (§6): the repository is a mirror Hulahoop
cloned from its url (`$DATA/mirrors/<repo-id>`), and the task has one jj workspace of
it at `$DATA/checkouts/<project-id>/<issue-id>`, reused across the task's turns and
deleted by a `cleanup` job once the task is Done or Cancelled. A checkout on the
user's machine is never borrowed, and there is no other runner mode. Artifacts live
under `$DATA/runs/<run-id>/` and outlive the checkout.

Why a workspace and not a clone per task: a workspace costs the working copy and
nothing else, and the runs of one repository share one object store. Keeping the
checkout across turns keeps what the project built, so a rework does not rebuild from
nothing.

One known limit: `just setup` and the format commands run with the runner's own
`HOME`, so toolchain caches such as `~/.cargo` are written there.

### Projects, milestones and artifacts

A workspace holds projects, a project holds repositories, and every issue is named by
its project.

| thing | what it is | where it lives |
| --- | --- | --- |
| project | a unit of work with a prefix (`BAL`, `ATL`) | `projects`, `projects.prefix` unique per workspace |
| issue key | `BAL-15`: prefix and number, stored, not computed on read | `issues.issue_key`, `issues.number` |
| projectless issue | planning that has no project yet, named by the workspace prefix | `issues.project_id` null, `workspaces.default_prefix` (`LOC` for a workspace named `Local`) |
| milestone | a name, a date and a body for a body of work | `milestones`, `issues.milestone_id` |
| artifact | bytes plus an index row; optionally filed under a milestone | `artifacts` table, `$DATA/artifacts/<id>/<filename>` on disk |

Three deliberate rules:

1. **An issue key is stored.** Renaming a project's prefix rewrites its issues' keys
   (`setProjectPrefix`), because a key is what people quote in a commit message and a
   ticket; recomputing it on read would silently rewrite history.
2. **A run follows the repository, and a task may have none.** A task filed under a
   project takes that project's first repository (`issues.repo_id`); a task with no
   project, or under a project with no repository, has `repo_id` null. Its runs still
   start: the runner gives the agent an empty private scratch directory
   (`$DATA/scratch/<run-id>`, deleted afterwards) with no clone, no repository lock,
   no preflight, no house checks, no diff and no pull request. The agent's output is
   its closing summary, its question and the files it leaves in
   `$HULAHOOP_ARTIFACT_DIR`; `HULAHOOP_VERDICT` / `HULAHOOP_QUESTION` move the task
   through the workflow as usual, and merging it is only the transition to Done.
   Repo-less runs take no repository slot, so several run at once (still one per
   task). `updateIssue(projectId:)` moves a task into a project (taking its first
   repository) or out of every project, while no run is live; the key is kept.
3. **An artifact is a file with an index row, and a download is authenticated.** The
   mutation takes base64 (`createArtifact`), the server writes
   `$DATA/artifacts/<id>/<filename>` and stores size, sha256, mime and kind; the
   desktop downloads through `GET /artifacts/<id>` with its bearer token, so no
   artifact is ever reachable without the token.

The screen side: a project picker in the rail (with `all projects`), keys on every
board card and issue title, `Milestones` and `Artifacts` screens, a milestone selector
on an issue, and a repository list per project in Settings — several repositories may
hang off one project, and the first is the one a run clones.

### Attaching a repository from GitHub

Settings lists every repository the installed GitHub app can reach, so a project can
be attached to a repository that has no checkout on this machine.

- **Listing needs `Metadata: Read`**, which every GitHub App has whether it asks or
  not. `Metadata` is enough to see names, owners and clone URLs; `Contents: Write` is
  what the loop already needs for pushing. `Checks: Read` and `Actions: Read` are
  display features for a later screen and stay optional.
- **The list goes through an installation token.** An app JWT can read the app and its
  installations; the repository list hangs off each installation, so the server lists
  installations first and then their repositories, one token at a time. An app with no
  installation gets a sentence saying so rather than an empty list.
- **Every repository is a mirror Hulahoop owns.** The server gives it a path under
  `$DATA/mirrors/<repo-id>`, the first job clones into it (with a token minted for that
  one repository when it is on GitHub), later runs fetch, and the loop never works
  inside a checkout the user is using.

### Workspaces

A workspace is the root of the tree: projects, milestones, artifacts and issues all
carry its id, and a server can hold several.

- **An issue with no project is still filed somewhere.** It belongs to the workspace
  and is keyed by the workspace prefix, which is derived from the name — a workspace
  called `Local` files `LOC-1`, `LOC-2` — and is editable in Settings. That is the key
  for work that spans every project in the workspace.
- **Prefixes are per workspace and non-overlapping.** A project prefix must be unique
  inside the workspace and must not equal the workspace prefix, or two sets of issues
  would share one key space; both directions are refused with the reason.
- **Changing a prefix rewrites the keys that already exist**, for the project
  (`setProjectPrefix`) and for the workspace (`updateWorkspace`). The key is what
  people quote in a commit message, so it is stored and rewritten rather than
  recomputed later.
- **The rail carries a workspace switcher** once a server has more than one, and
  Settings creates them: name plus an optional prefix, defaulting from the name
  (`Bali` → `BAL`). Switching reloads projects, issues, milestones and artifacts, and
  the choice is remembered between reloads.

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
3. **A `runner_jobs` table** for fetch, preflight, landing, checkout cleanup and
   forgetting a mirror: each is a piece of VCS or file work the runner does in the
   data directory, claimed over the internal API.
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
9. **The runner takes the repository from the project**, not from a constant. A
   project may hold several repositories; the issue records which one it is tested
   against (`issues.repo_id`), and the clone URL comes from that repository's url,
   never from a hardcoded default.
10. **The desktop connects to a URL, not a port.** The first build wired the app to
   `http://127.0.0.1:<port>`; the server now advertises a `url`, the shell passes it
   through, and a saved host in `localStorage` wins over the bundled one. Without that
   the app could only ever be a single-machine tool.

### What is not built yet

- Merging a pull request from inside Hulahoop: the human merges on GitHub and
  Hulahoop reads it. A project without pull requests is merged by Hulahoop itself,
  which pushes the reviewed head to the base branch.
- `bwrap` and `sandbox-exec` profiles (M1). The v0 boundary is the environment
  allowlist plus a scratch `HOME`; `~/.ssh`, `~/.aws` and the rest are never passed.
- Linux desktop builds and Windows. The flake builds and tests on Linux, the bundle
  path targets macOS.
- Hosted mode: one server per person, one bearer token, no users, roles, or TLS.
  Remote mode is "point the desktop at your own workstation", which is what §18 above
  documents; the hosted variant with Postgres and per-user auth is M3.
- The macOS artifact: `.dmg` bundling runs in GitHub Actions
  (`.github/workflows/release.yml`) on a `v*` tag, unsigned, with the quarantine
  instructions from [13. Open source and delivery](#13-open-source-and-delivery).
  The workflow ships Apple silicon only: GitHub's hosted x86_64 macOS image is
  retired, so an Intel build needs a larger runner or a local `just bundle
  x86_64-apple-darwin`.

---

## 18b. How a status runs its team

A status names its team on one line, and the order is the order of turns:

```markdown
## To do

Users: @Architect, @Test Author
Next: In progress - the plan and the criteria are agreed.
```

`@Architect` works first. When its run succeeds, `@Test Author` picks the same
status up with the plan in its prompt; the task only moves when nobody is owed a
turn. A move that stays inside the status ("the reviewer asks for changes")
restarts the queue, and the agents share context through comments on the task.

`Turns:` fences a status that keeps passing the topic around. It lives in the same
document, defaults to six, may be overridden per section, and reaching it moves
the task to `Blocked` with a comment naming who kept handing back.

```markdown
# Workflows

Turns: 6
```

Nothing else is needed: no handoff marker, no assignment screen. Edit the
document and the execution changes with it.

## 19. The loop as it ships

Six statuses, and nothing else:

| status | owner | what happens |
| --- | --- | --- |
| Backlog | person | ideas you have not committed to. Nothing is dispatched |
| To do | agent | entering this column **is** the dispatch: an agent starts |
| In progress | agent | the agent works, opens a pull request, and hands over |
| Blocked | person | the agent asked a question; answer it and the task resumes |
| In review | person | the pull request is open and its checks have run |
| Done | person | the pull request merged |

The workflow is a document. `WORKFLOWS.md` in the workspace, one `## Status`
heading per column, and two kinds of line under each:

```markdown
## To do

Users: @TestAuthor
Next: In progress - when the tests are written
Next: Blocked - when a decision is needed, and the agent cannot make it

Ready for the team. Anything here is body text.
```

`Users` is the only line about people: `human` (or `you`) for a column a person
works, an agent's name for a column an agent works, `any` for the agent whose last
run is oldest, and a mix when both may act. One line, comma separated, `@`
optional. A column with no `Users` line belongs to a person, so a half-written
document never dispatches an agent nobody named. A blocked column and a finished
one start nothing whatever they name: the first waits for a person, and the second
is over.

Each `Next:` is one move and its reason, written `Next: <status> - <when>`. The
reason may contain commas; only the part before the dash is a status. A column may
have as many moves as the team needs, including back to itself, and a column with
no `Next:` is final. The parser refuses a document that names an unknown status,
omits a column, or leaves a `Next` pointing nowhere, because a board that silently
drops a column lies about where work can go.

An agent is its own knowledge: an instructions text, its `AGENTS.md`, and the
skills attached to it. Skills are workspace knowledge — a convention, a checklist,
a house rule with the story behind it — written once and carried by whichever
agents need them.

What a run leaves behind: a workspace of the repository at the current base, a
transcript in `agent.jsonl`, the diff, the token count, and a pull request. What
closes a task is that pull request merging; nothing else moves a task to Done.

What the loop keeps from the frozen-suite years: one workspace per run, the house
rules (`just fmt-check`, `just lint`) checked at the same revision as the suite,
the pull-request check gate, and the transcripts. What it dropped: the separate
Test Author, frozen test files, replays, the diff-hash approval, and the per-project
agent bindings — a run says what it did by what it left behind, so nothing has to
record what kind of turn it was.

Acceptance criteria went the same way: a task is its title and body, and
migration 0022 folded every existing criterion into its task's body as a
`## Acceptance criteria` checklist. What a turn reads instead is the task, the
files attached to it, and what people and Lumi said since the previous turn
started. A comment can mention `@Agent` or `@Lumi`; an agent mention never goes
around `WORKFLOWS.md` — it starts a turn only when the current status already
owes that agent one and nothing is in flight, and otherwise it is a note the agent
reads next time. Due date and assignee are for people: an agent becomes the
assignee when it starts a turn, and neither field decides who works.
