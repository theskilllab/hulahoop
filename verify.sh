#!/usr/bin/env bash
# Revision-0 gate for README.md. One predicate per ledger row.
set -uo pipefail
f="${1:-README.md}"
fail=0
pass=0

has() { if grep -qF -- "$2" "$f"; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL #$1 missing: $2"; fi; }
hasnt() { if grep -qF -- "$2" "$f"; then fail=$((fail+1)); echo "FAIL #$1 stale: $2"; else pass=$((pass+1)); fi; }

# 1 done reachable, merging state, recordMerge mutation
has 1 "recordMerge"
has 1 "merging"
hasnt 1 "done         Reviewer approved"
# 2 blocked returns to the status it left
has 2 "status_before_blocked"
# 3 transition table
has 3 "### Issue transitions"
# 4 sandbox grants the checkout it denies the data dir for
has 4 'or the rest of `$DATA`'
# 5 repo lock has one owner, acquired over the internal API
has 5 "/internal/repos/:id/lock"
# 6 the server owns the log file
hasnt 6 "Streams logs and artifacts"
has 6 "the server writes the log file"
# 7 jj handles are bookmarks, not bare commit hashes
has 7 "tests_bookmark"
has 7 "head_bookmark"
# 8 .envrc is generated when absent
has 8 "generated when absent"
# 9 checkout path carries the run id
has 9 "checkouts/<project>/<issue-id>/<owner-id>"
# 10 worktree branch is unique per attempt
has 10 "issue/<id>/<kind>-<owner-id>"
# 11 prompts ship with the app
has 11 "embedded prompt set"
# 12 ignores are written, not assumed
has 12 ".git/info/exclude"
# 13 artifacts have a directory, replays have their own
has 13 "artifact_dir"
has 13 "replays/<replay-id>/"
# 14 role and active-run guards
has 14 "one queued or running run per issue"
# 15 project setup mutations
has 15 "addRepo"
has 15 "setProjectAgents"
# 16 no runLog subscription
hasnt 16 'runChanged`, `runLog'
# 17 stacked run is human started
has 17 "the human starts the stacked run"
# 18 v0 sandbox scope matches M0
has 18 "and a clean environment in v0"
# 19 retries are a distinct M2 feature
has 19 'jj checkout create/diff/cleanup. The git worktree provider ships in M1'
# 20 stale text, cache fallback, circular key
has 20 "cachix/cachix-action"
hasnt 20 "three prompts with two agents"
hasnt 20 "checkout_id, parent_run_id"

# --- audit round 2 (blind reader), ledger rows 21-45 ---
# 21 one row closes an issue
has 21 'recordMerge(mergeRevision)` | a green landing replay for this approval exists'
hasnt 21 'green | `junit-base.xml` red'
# 22 reviewer has transitions
has 22 'startRun(role: reviewer)'
has 22 'reviewer verdict `changes_requested`'
# 23 blocked reachable from every active state, with a writer
has 23 'agent ends its run with a question'
has 23 'pinned_comment_id'
# 24 freezeSuite owns tests_ready
has 24 'freezeSuite(issueId)` | base replay present'
# 25 no resolved hash reaches jj
has 25 'the `tests/<issue>` bookmark for the Implementer'
hasnt 25 'HULAHOOP_TESTS_REVISION'
# 26 comments are v0
has 26 'comments, which carry a blocking question and its answer'
hasnt 26 'Add issue comments.'
# 27 thaw is v0 only
hasnt 27 'Thaw and re-freeze a suite without losing'
hasnt 27 'Add thaw and re-freeze.'
# 28 confinement is M1
has 28 'Process-level confinement arrives in M1'
has 28 'bwrap` and `sandbox-exec` profiles for enforced confinement'
# 29 credentials survive the scratch HOME
has 29 '~/.claude/.credentials.json'
# 30 preflight needs two files
has 30 'A supported repo declares two files'
hasnt 30 'declares three files'
# 31 milestone duplication gone
hasnt 31 'Connection file and desktop sidecar spawn'
hasnt 31 'Role prompts and skills.'
hasnt 31 'Orphan checkout cleanup.'
# 32 merge commit is stored
has 32 'merge_revision: the commit recordMerge accepted'
# 33 the approval pins the diff it claims to pin
has 33 'diffSha: String!'
has 33 'diff_sha: sha256 of diff.patch'
# 34 internal routes and their token
has 34 '/internal/runs/:id/artifacts'
has 34 '/internal/replays/claim'
has 34 'Authorization: Bearer <token>'
# 35 excludes work in every repo type
has 35 '~glob:.hulahoop/**'
# 36 dead risk rows gone
hasnt 36 'cut the diff view to plain text'
hasnt 36 '| jj concurrency |'
# 37 enum casing is mapped once
has 37 'TESTS_IN_PROGRESS` ↔ `tests_in_progress`'
# 38 the reason an issue left is stored
has 38 'cancelled` shows `runs.error'
# 39 object types exist
has 39 'type Run {'
has 39 'run(id: ID!): Run'
# 40 replays are rows, not runs
has 40 'replays               one row per suite execution'
# 41 M0 migrations cover the loop
has 41 'replays, reviews, comments, approvals, repo_locks, run_events'
# 42 M0 screens and mutations
has 42 'Workspace/project/repo/agent setup with preflight'
has 42 'Local notifications for `in_review`, `blocked`, and `merging`'
# 43 flags have storage
has 43 'flags: touched_contract | large_diff | artifact_limit'
# 44 the run cap can be cleared
has 44 'clearRunCap(issueId: ID!)'
has 44 'run_cap_cleared_at'
# 45 restart recovery has fields
has 45 'runs.runner_id` and `runs.claimed_at`'
has 45 'state: creating | ready | cleaning | removed'
# 46 the rework loop has a legal transition
has 46 '| `in_progress` | `startRun(role: implementer, parentRunId)` |'
has 46 'the same `startRun` reached from `in_progress` with `parentRunId`'
# 47 thaw can start another authoring run
has 47 '| `tests_in_progress` | `startRun(role: test_author)` |'
# 48 cancel and abandon are different
has 48 '`cancelRun(runId)` | process group killed; the issue keeps its status'
has 48 'abandonIssue(reason)'
hasnt 48 'cancelRun(runId)` | human, process group killed | `cancelled`'
# 49 blocking does not leave a live run
has 49 'ends its run with a question'
has 49 'never holds a live process'
# 50 checkouts are keyed by owner
has 50 'owner_kind: run | replay'
has 50 'One checkout per active run, replay, or preflight'
hasnt 50 'One checkout per active run.'
# 51 attempts are stored
has 51 'attempt, suite_seq, run_count'
has 51 'attempt: increments per implementation run'
# 52 criteria map to tests
has 52 'id, issue_id, position, text, test_name'
has 52 'one criterion maps to exactly one test'
hasnt 52 'acceptance, status, priority'
# 53 approvals can be voided
has 53 'voided_at: set when a new run starts'
has 53 'runId: ID!'
# 54 the reviewer can report a verdict
has 54 'POST /internal/runs/:id/verdict'
has 54 'reviews` row and fires the transition'
# 55 the contract version is a build constant
has 55 'CONTRACT_VERSION = "26.11"'
hasnt 55 'contract_version: 26.11'
# 56 the reviewer login persists outside the checkout
has 56 '$DATA/home/<adapter>'
has 56 'a Reviewer logs in once and every later run'
# 57 replays handle a timeout
has 57 '`timed_out` is neither'
# 58 the base handle is defined
has 58 'that pair is "the current base"'
# 59 preflight runs inside the dev shell
has 59 'direnv exec . pi --version'
has 59 'only exists inside the flake passes'

# 60 criteria map to test names by convention
has 60 'gets exactly one test whose name carries its position'
has 60 'requires exactly one match per criterion'
has 60 'writes it into `criteria.test_name`'

# 61 the landing replay is the revision that gets merged
has 61 'writes `landing/<issue>/<attempt>` at the rebased revision'
has 61 'POST /internal/replays/:id/ancestry'
has 61 'ancestry_ok: written by the landing replay'
# 62 rework is reachable again, current run is explicit
has 62 'current_run_id: the implementer run a human may approve'
has 62 'runId` is `current_run_id`'
hasnt 62 'base_bookmark is the issue'
# 63 approval needs a reviewer verdict
has 63 'that run has an `approved` review'
# 64 base replay applies the authored tests
has 64 'jj restore --from <that bookmark>'
has 64 'would run an empty diff and prove nothing'
# 65 frozen paths are stored
has 65 'frozen_paths: the files the authoring run wrote'
has 65 'touches any path in `issues.frozen_paths`'
# 66 criteria names match at a token boundary
has 66 'ac12([^0-9]|$)'
has 66 'lets `ac1` match `ac10`'
# 67 question beats completion
has 67 'fires the `blocked` row and does **not** fire the completion row'
# 68 resume carries the answer
has 68 'resumeIssue(issueId: ID!, answer: String!)'
has 68 'writes the answer as a comment'
# 69 preflight has its own checkout and per-repo keying
has 69 'owner_kind = preflight'
has 69 'keyed by `(agent_id, repo_id)`'
hasnt 69 'preflight_ok'
# 70 one agent per repo is enforced
has 70 'one_active_run_per_repo'
# 71 replay rows identify their approval and suite
has 71 'approval_id: set on a landing replay'
has 71 'tests_bookmark: the authored suite the replay applied'
# 72 one Query block and no SuiteRun
hasnt 72 'type SuiteRun'
hasnt 72 'suiteRuns(issueId'
# 73 the v0 confinement gap is a recorded risk
has 73 'v0 has no filesystem confinement'
# 74 thaw archives by suite sequence
has 74 'tests/<issue>/archive/<suite_seq>'
has 74 'suite_seq: increments per authored suite'
# 75 the git provider does not ship in M0
has 75 'The git worktree provider ships in M1'

has 131 "BAL-15" "an issue key names its project prefix"
has 132 "issues.issue_key" "the key is stored, not derived"
has 133 "Milestones" "a milestone screen exists"
has 134 "artifacts/<id>/<filename>" "artifacts live on disk under their id"
has 135 "GET /artifacts/<id>" "artifact downloads are authenticated"

echo "---"
# ---- workspaces and prefixes ----------------------------------------------
has 136 "LOC-1" "the workspace prefix is derived from the name"
has 137 "must not equal the workspace prefix" "a project may not take the workspace prefix"
has 138 "Changing a prefix rewrites the keys that already exist" "renaming a prefix rewrites keys"
has 139 "workspace switcher" "the rail switches workspaces"

# ---- repositories from GitHub ---------------------------------------------
has 140 "Attaching a repository from GitHub" "the GitHub repository list is documented"
has 141 "Metadata: Read" "listing needs metadata read"
has 142 "mirror, cloned on the first run" "a remote-only repository is a mirror"

echo "pass=$pass fail=$fail"
[ "$fail" -eq 0 ]


has 132 "issues.issue_key" "the key is stored, not derived"
has 133 "Milestones" "a milestone screen exists"
has 134 "artifacts/<id>/<filename>" "artifacts live on disk under their id"
has 135 "GET /artifacts/<id>" "artifact downloads are authenticated"

