# Revision 0 ledger

One row per defect found in `README.md`. `Predicate` is what `./verify.sh` asserts; `Status` is `done` only when the predicate passes. Round 2 rows came from a blind reader that never saw this file.

| # | Defect | Section | Predicate | Status | Correction applied |
| --- | --- | --- | --- | --- | --- |
| 1 | `done` unreachable through the API | §2, §5, §10 | has(`merging`) has(`recordMerge`) hasnt(`in_progress  Reviewer requested changes`) | done | `merging` status, `recordMerge`, `### Issue transitions` |
| 2 | `blocked` has no return path | §5 | has(`status_before_blocked`) | done | column plus resume guard |
| 3 | no transition table | §5 | has(`### Issue transitions`) | done | server-side table with guards |
| 4 | sandbox denies the directory holding checkouts | §6 | has(`or the rest of \`$DATA\``) | done | deny clause carries the two exceptions |
| 5 | `repo_locks` claimed by two writers | §3, §4 | has(`/internal/repos/:id/lock`) | done | the server is the only writer; runner asks over HTTP |
| 6 | log file has no owner | §3, §5 | hasnt(`Streams logs and artifacts`) has(`the server writes the log file`) | done | runner posts bytes, server writes |
| 7 | jj commit hashes are unstable handles | §5, §6 | has(`tests_bookmark`) has(`head_bookmark`) | done | bookmarks are the handles |
| 8 | `.envrc` both required and generated | §2, §7 | has(`generated when absent`) | done | required: `flake.nix` + `justfile`; `.envrc` generated |
| 9 | checkout path has no run id | §6 | has(`checkouts/<project>/<issue-id>/<owner-id>`) | done | path and workspace name carry the run id |
| 10 | `git worktree add -b` collides on retry | §6 | has(`issue/<id>/<attempt>`) | done | attempt number in the branch |
| 11 | `prompt_revision` only resolves inside Hulahoop's own repo | §5, §8 | has(`embedded prompt set`) | done | prompts embedded in the binary, hash covers them |
| 12 | "excluded from the diff" has no mechanism | §6 | has(`.git/info/exclude`) | done | git excludes, superseded by row 35 |
| 13 | gate artifacts have no home | §3, §5 | has(`artifact_dir`) has(`replays/<replay-id>/`) | done | `runs/` and `replays/` layouts, artifact route |
| 14 | `startRun` accepts any agent, active-run rule unenforced | §5 | has(`one queued or running run per issue`) | done | partial unique index plus role validation |
| 15 | screen 1 mutations missing | §10 | has(`addRepo`) has(`setProjectAgents`) | done | setup mutations listed |
| 16 | `runLog` listed as a GraphQL subscription | §4 | hasnt('runChanged`, `runLog') | done | `runChanged` plus a dedicated log socket |
| 17 | stacked run starts automatically | §8 | has(`the human starts the stacked run`) | done | human starts the retry with a review body |
| 18 | sandbox promise vs M0 contents | §2, §12 | has(`and a clean environment in v0`) | done | v0 scope stated, confinement moved to M1 |
| 19 | M0/M1 duplicate cancel, diff viewer; M2 retries duplicate stacked runs | §12 | has(`Retry an implementation attempt by starting a run from the issue`) | done | cancel and diff view only in M0 |
| 20 | stale risk text, `magic-nix-cache` fallback, missing defaults, `runs.checkout_id` circular | §3, §5, §13, §14 | has(`cachix/cachix-action`) hasnt(`three prompts with two agents`) hasnt(`checkout_id, parent_run_id`) | done | rows rewritten, circular key dropped, defaults added |
| 21 | two `merging → done` rows, one of them without `recordMerge` | §5 | has('recordMerge(mergeRevision)` \| landing replay green') hasnt('green \| `junit-base.xml` red') | done | single `done` row behind `recordMerge`; the red row returns to `in_progress` |
| 22 | Reviewer had no transition, though its role exists | §5, §10 | has(`startRun(role: reviewer)`) has(`reviewer verdict ` + "`" + `changes_requested`) | done | reviewer rows leave the status until a verdict lands |
| 23 | `blocked` over-claimed, no writer for `status_before_blocked` | §5 | has('any active \| agent ends its run with a question') has(`pinned_comment_id`) | done | blocking question is a pinned comment from any active state |
| 24 | `freezeSuite` absent from the table | §5, §8 | has('freezeSuite(issueId)` \| base replay present') | done | `freezeSuite` owns `tests_ready`, replay is only evidence |
| 25 | bookmark fix half-applied: resolved hashes still reached `jj` | §6, §8 | has('the ` + "`" + `tests/<issue>` + "`" + ` bookmark for the Implementer') hasnt(`HULAHOOP_TESTS_REVISION`) | done | bookmark names in `jj workspace add`, env vars renamed |
| 26 | comments simultaneously v0 and M1 | §2, §9, §11, §12 | has(`comments, which carry a blocking question and its answer`) hasnt(`Add issue comments.`) | done | comments ship in v0; the M1 and dogfood entries are gone |
| 27 | thaw duplicated in v0, M1, and the dogfood list | §12, §11 | hasnt(`Thaw and re-freeze a suite without losing`) hasnt(`Add thaw and re-freeze.`) | done | thaw stays a v0 feature |
| 28 | sandbox block unqualified while M1 claims the profiles | §6, §12 | has(`Process-level confinement arrives in M1`) has('bwrap` and `sandbox-exec` profiles for enforced confinement') | done | v0 states its boundary honestly, M1 adds enforcement |
| 29 | Claude Code cannot authenticate inside the scratch `HOME` | §6, §8 | has(`~/.claude/.credentials.json`) | done | scratch `HOME` seeded with the adapter's credential file |
| 30 | M0 demanded `.envrc`, §7 generates it | §7, §12 | has(`A supported repo declares two files`) hasnt(`declares three files`) | done | two required files, generated shell entry |
| 31 | milestone duplication survived (connection file, role prompts, orphan cleanup) | §12 | hasnt(`Connection file and desktop sidecar spawn`) hasnt(`Role prompts and skills.`) hasnt(`Orphan checkout cleanup.`) | done | M1 and M2 list only what M0 does not do |
| 32 | no column for the merge commit | §5, §10 | has(`merge_revision: the commit recordMerge accepted`) | done | `merge_revision`, `merged_at` on `issues` |
| 33 | `diff_sha` written by nobody, checked by nothing | §5, §10 | has(`diffSha: String!`) has(`diff_sha: sha256 of diff.patch`) | done | `approveIssue` takes and verifies it |
| 34 | internal API has no artifact route, no auth | §4 | has(`/internal/runs/:id/artifacts`) has(`/internal/replays/claim`) has(`Authorization: Bearer <token>`) | done | artifact/spec/replay routes plus a token check; the token never reaches an agent |
| 35 | `.git/info/exclude` does not exist in a pure jj repo | §6 | has(`~glob:.hulahoop/**`) | done | fileset exclusion on every diff, git excludes as a bonus |
| 36 | dead and duplicate risk rows | §14 | hasnt(`cut the diff view to plain text`) hasnt('\| jj concurrency \|') | done | one jj row, diff row states the decision instead of offering a cut |
| 37 | `merging` vs `MERGING` casing drift | §5 | has('TESTS_IN_PROGRESS` ↔ `tests_in_progress') | done | one mapping line, one table in code |
| 38 | "the reason it left" had no field | §5, §9 | has('cancelled` shows `runs.error') | done | pinned comment for `blocked`, `runs.error` for `cancelled` |
| 39 | §10 declared no object types, no `run(id:)` | §10 | has(`type Run {`) has(`run(id: ID!): Run`) | done | every screen field has a type |
| 40 | a replay could not exist as a `runs` row | §5, §8 | has(`replays               one row per suite execution`) | done | `replays` table, kind base/head/landing, own artifacts |
| 41 | M0 migrations omitted the tables its exit needs | §12 | has(`replays, reviews, comments, approvals, repo_locks, run_events`) | done | full table list |
| 42 | M0 omitted screen 1, run detail, notifications, `recordMerge`, resume | §12 | has(`Workspace/project/repo/agent setup with preflight`) has(`Local notifications for \`in_review\`, \`blocked\`, and \`merging\``) | done | M0 list rebuilt around the exit criterion |
| 43 | nothing stored the flags the gates raise | §5, §10 | has(`flags: touched_contract \| large_diff \| artifact_limit`) | done | `runs.flags` plus `acknowledgeFlags` on approval |
| 44 | clearing the run cap was undefined | §5, §10 | has(`clearRunCap(issueId: ID!)`) has(`run_cap_cleared_at`) | done | mutation plus column |
| 45 | restart recovery had no fields | §5, §12 | has('runs.runner_id` and `runs.claimed_at') has(`state: creating \| ready \| cleaning \| removed`) | done | claim identity, claim time, checkout states |

| 46 | the rework loop had no legal transition | §5 | has('in_progress` | `startRun(role: implementer, parentRunId)') has(`the same \`startRun\` reached from \`in_progress\` with \`parentRunId\``) | done | one rework row, three paths converge on it |
| 47 | thaw was a trap: no authoring run from `tests_in_progress` | §5 | has('tests_in_progress` | `startRun(role: test_author)') | done | thaw path row |
| 48 | cancel had two destinations, one of them terminal | §5, §10 | has(`the issue keeps its status`) has(`abandonIssue(reason)`) | done | `cancelRun` stops a process, `abandonIssue` closes an issue |
| 49 | `blocked` could not be escaped while a run lived | §5 | has(`ends its run with a question`) has(`never holds a live process`) | done | a question ends the run, so no live process blocks resume |
| 50 | checkout cardinality was impossible | §5, §6 | has(`owner_kind: run \| replay`) has(`One checkout per active run or replay`) | done | checkouts keyed by owner, replays own theirs |
| 51 | no attempt counter existed | §5, §6 | has(`attempt: increments per implementation run`) has(`runs.attempt`) | done | `issues.attempt`, `runs.attempt`, attempt in every handle |
| 52 | criteria-to-test mapping was stored nowhere | §5 | has(`id, issue_id, position, text, test_name`) | done | `criteria` table, the freeze gate reads it |
| 53 | approvals could never be voided, `runId` unnameable | §5, §10 | has(`voided_at: set when a new run starts`) has(`runId: ID!`) | done | `voided_at` plus `runId` on the mutation |
| 54 | the Reviewer had no write path for a verdict | §4, §10 | has(`POST /internal/runs/:id/verdict`) | done | verdict route writes the review and fires the transition |
| 55 | `contract_version` had no source and no owner | §5, §7 | has(`CONTRACT_VERSION = ""26.11""`) | done | a build constant the build compares against |
| 56 | the Reviewer's login died with its checkout | §6 | has(`$DATA/home/<adapter>`) | done | scratch home per adapter, outside the checkout |
| 57 | a timed-out replay was neither red nor green | §5 | has(`\`timed_out\` is neither`) | done | the gate says what it does with each status |
| 58 | "current base" was never defined or updated | §6 | has(`that pair is "the current base"`) | done | fetch writes base and `fetched_at`, merge updates base |
| 59 | preflight probed the host PATH while agents run in the dev shell | §8 | has(`direnv exec . pi --version`) has(`only exists inside the flake passes`) | done | probes run through `direnv exec`, results stored on `agents` |

| 60 | nothing connected a criterion to a test name | §2, §5 | has(`gets exactly one test whose name carries its position`) has(`writes the match into \`criteria.test_name\``) | done | `ac<position>` in the test name, gate parses it from JUnit |

| 61 | the landing replay verified a tree nobody merges | §2, §5 | has(`writes \`landing/<issue>/<attempt>\` at the rebased revision`) has(`ancestry_ok: written by the landing replay`) | done | landing bookmark, ancestry route, `recordMerge` reads the row |
| 62 | `changes_requested` was a dead end, `currentRun` pinned the wrong attempt | §5, §10 | has(`current_run_id: the implementer run a human may approve`) has('runId` is `current_run_id`') | done | explicit `current_run_id`, base-bookmark clause dropped |
| 63 | the three-role handoff was decorative: approval ignored the verdict | §5, §9 | has('that run has an `approved` review') | done | guard requires it, the button disables without it |
| 64 | the base replay ran tests that do not exist at base | §6, §8 | has('jj restore --from <that bookmark>') | done | candidate bookmark, tests restored onto the base checkout |
| 65 | the frozen-file guard had no file set | §5, §8 | has(`frozen_paths: the files the authoring run wrote`) | done | `issues.frozen_paths`, written by `freezeSuite` |
| 66 | `ac1` matched `ac10` | §2 | has(`ac12([^0-9]|$)`) has('lets `ac1` match `ac10`') | done | token-boundary match, distinct names required |
| 67 | a question and a completion fired two rows | §5 | has('fires the `blocked` row and does **not** fire the completion row') | done | precedence stated, replay-while-blocked defined |
| 68 | `resumeIssue` could not carry the answer | §5, §10 | has('resumeIssue(issueId: ID!, answer: String!)') | done | answer argument writes the comment and clears the pin |
| 69 | preflight wrote into the user's repo and was keyed per agent | §8, §10 | has('owner_kind = preflight') has('keyed by `(agent_id, repo_id)`') | done | throwaway checkout, `agent_preflight` table |
| 70 | one agent per repo was a sentence in the risk table | §5, §14 | has(`one_active_run_per_repo`) | done | partial unique index on `runs(repo_id)` |
| 71 | a stale green landing replay could unlock a new merge | §5 | has('approval_id: set on a landing replay') has('tests_bookmark: the authored suite the replay applied') | done | replay rows identify approval, attempt, and suite |
| 72 | two `type Query` blocks, `SuiteRun` duplicated `replays` | §10 | hasnt(`type SuiteRun`) hasnt(`suiteRuns(issueId`) | done | one Query, suite_runs table dropped |
| 73 | v0 isolation was claimed more strongly than delivered | §4, §14 | has('v0 has no filesystem confinement') | done | the `$DATA` read gap is a recorded risk |
| 74 | a second thaw in one attempt destroyed the first archive | §5, §6 | has('tests/<issue>/archive/<suite_seq>') | done | `suite_seq` replaces `attempt` in archives |
| 75 | the git worktree provider collided on branch names, and M0 claimed it | §6, §12 | has('The git worktree provider ships in M1') has('issue/<id>/<kind>-<owner-id>') | done | one branch per owner, git provider moved to M1 |

## Verification

```bash
./verify.sh          # 130 predicates, one per row, exit 0 when the file agrees
```

Baseline before revision 0: `/tmp/readme.base.md`.
