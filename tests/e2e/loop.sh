#!/usr/bin/env bash
# Runs the whole loop once against a throwaway repo with the fake adapter:
# criteria → authored tests → frozen suite → implementation → review → approval
# → landing replay → merge → done. Prints the status at every step, so a failure
# points at the step that moved (or did not).
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT="$PWD"
WORK="$(mktemp -d /tmp/hulahoop-e2e.XXXXXX)"
REPO="$WORK/repo"
DATA="$WORK/data"
mkdir -p "$REPO" "$DATA"

echo "work dir: $WORK"

# One file for the whole run, so the driver can talk to the server.
CONN="$DATA/connection.json"
port() { jq -r .port "$CONN"; }
token() { jq -r .token "$CONN"; }
gql() {
  local query="$1"
  shift
  curl -sf -X POST "http://127.0.0.1:$(port)/graphql" \
    -H 'content-type: application/json' \
    -H "authorization: Bearer $(token)" \
    --data "$(jq -cn --arg q "$query" --argjson v "$1" '{query:$q, variables:$v}')"
}

copy_fixture() {
  cp -r fixtures/repo/. "$REPO/"
  git -C "$REPO" init -q -b main
  git -C "$REPO" add -A
  git -C "$REPO" -c user.email=e2e@example.test -c user.name=e2e commit -qm "initial"
  # jj imports the git commit and puts the working copy on top of it; the
  # bookmark belongs on the imported commit, which is `@-`.
  ( cd "$REPO" && jj git init --colocate >/dev/null 2>&1 || true )
  ( cd "$REPO" && jj bookmark create main -r @- 2>/dev/null || jj bookmark set main -r @- )
}

wait_status() {
  local issue_id="$1" wanted="$2" budget="${3:-90}"
  for _ in $(seq 1 "$budget"); do
    local status
    status=$(gql 'query ($id: UUID!) { issue(id: $id) { status } }' "{\"id\":\"$issue_id\"}" | jq -r '.data.issue.status')
    if [[ "$status" == "$wanted" ]]; then
      echo "  → $wanted"
      return 0
    fi
    if [[ "$status" == "blocked" || "$status" == "cancelled" ]]; then
      echo "  ✗ stopped at $status (wanted $wanted)" >&2
      gql 'query ($id: UUID!) { issue(id: $id) { comments { authorType body } } }' "{\"id\":\"$issue_id\"}" | jq -r '.data.issue.comments[] | "    [\(.authorType)] \(.body)"' >&2
      return 1
    fi
    sleep 1
  done
  echo "  ✗ timed out waiting for $wanted" >&2
  return 1
}

cleanup() {
  [[ -n "${RUNNER_PID:-}" ]] && kill "$RUNNER_PID" 2>/dev/null || true
  [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

copy_fixture
echo "repo prepared at $REPO"

# CI runs this after `cargo test`, which builds test targets but not the binaries.
for bin in hulahoop; do
  if [[ ! -x "$ROOT/target/debug/$bin" ]]; then
    echo "building $bin"
    (cd "$ROOT" && cargo build --bin "$bin") || { echo "could not build $bin" >&2; exit 1; }
  fi
done

HULAHOOP_DATA="$DATA" "$ROOT/target/debug/hulahoop" serve --data-dir "$DATA" --listen 127.0.0.1 >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 100); do [[ -f "$CONN" ]] && break; sleep 0.1; done
[[ -f "$CONN" ]] || { echo "server did not start" >&2; cat "$WORK/server.log" >&2; exit 1; }

# The runner is pointed at the server the way a remote host would be: an address
# and a token, no shared directory. Remote mode then gets exercised on every run.
SERVER_URL=$(jq -r .url "$CONN")
[[ -n "$SERVER_URL" && "$SERVER_URL" != "null" ]] || SERVER_URL="http://127.0.0.1:$(jq -r .port "$CONN")"

HULAHOOP_DATA="$DATA" HULAHOOP_FAKE_SCRIPT="$ROOT/tests/e2e/fake_agent.sh" \
  "$ROOT/target/debug/hulahoop" runner --data-dir "$DATA" --server "$SERVER_URL" --token "$(jq -r .token "$CONN")" \
  >"$WORK/runner.log" 2>&1 &
RUNNER_PID=$!

workspace=$(gql '{ workspaces { id } }' '{}' | jq -r '.data.workspaces[0].id')
project=$(gql 'mutation ($id: UUID!) { createProject(workspaceId: $id, name: "e2e") { id } }' "{\"id\":\"$workspace\"}" | jq -r '.data.createProject.id')
repo=$(gql 'mutation ($projectId: UUID!, $name: String!, $path: String!) { addRepo(projectId: $projectId, name: $name, path: $path) { id } }' "{\"projectId\":\"$project\",\"name\":\"repo\",\"path\":\"$REPO\"}" | jq -r '.data.addRepo.id // .errors')
echo "project $project / repo $repo"

agent() {
  gql 'mutation ($workspaceId: UUID!, $name: String!, $adapter: String!, $model: String!) {
         createAgent(workspaceId: $workspaceId, name: $name, adapter: $adapter, model: $model) { id }
       }' "{\"workspaceId\":\"$workspace\",\"name\":\"$1\",\"adapter\":\"$2\",\"model\":\"$3\"}" | jq -r '.data.createAgent.id'
}
# Two configurations of the same loop: the fake adapter proves the plumbing on every
# change, the real ones prove the product. HULAHOOP_E2E_ADAPTER=real picks the latter.
if [[ "${HULAHOOP_E2E_ADAPTER:-fake}" == "real" ]]; then
  # A real agent reads the repo, edits, and runs the suite; the fake one answers in
  # milliseconds. Only the wait budget changes between the two.
  SLOW_WAIT="${SLOW_WAIT:-900}"
  # Empty model means "whatever the adapter is configured for". Claude Code is the
  # adapter the spec wants for the Reviewer, but it needs a login, so a machine
  # without one runs the same review through pi and a different model.
  PI_MODEL="${PI_MODEL:-}"
  REVIEWER_MODEL="${REVIEWER_MODEL:-}"
  REVIEWER_ADAPTER="${HULAHOOP_REVIEWER_ADAPTER:-pi}"
  tester=$(agent tester pi "$PI_MODEL")
  implementer=$(agent implementer pi "$PI_MODEL")
  reviewer=$(agent reviewer "$REVIEWER_ADAPTER" "$REVIEWER_MODEL")
else
  tester=$(agent tester fake "fake-test-author")
  implementer=$(agent implementer fake "fake-implementer")
  reviewer=$(agent reviewer fake "fake-reviewer")
fi

gql 'mutation ($projectId: UUID!, $bindings: [RoleBinding!]!) { setProjectAgents(projectId: $projectId, bindings: $bindings) { role } }' \
  "{\"projectId\":\"$project\",\"bindings\":[{\"role\":\"test_author\",\"agentId\":\"$tester\"},{\"role\":\"implementer\",\"agentId\":\"$implementer\"},{\"role\":\"reviewer\",\"agentId\":\"$reviewer\"}]}" >/dev/null

echo "preflight"
for agent_id in "$tester" "$implementer" "$reviewer"; do
  gql 'mutation ($agentId: UUID!, $repoId: UUID!) { runPreflight(agentId: $agentId, repoId: $repoId) }' \
    "{\"agentId\":\"$agent_id\",\"repoId\":\"$repo\"}" >/dev/null
done
for _ in $(seq 1 60); do
  green=$(gql 'query ($id: UUID!) { preflight(projectId: $id) { ok } }' "{\"id\":\"$project\"}" | jq '[.data.preflight[] | select(.ok)] | length')
  [[ "$green" == "3" ]] && break
  sleep 1
done
echo "  → $green/3 green"
if [[ "$green" != "3" ]]; then
  echo "preflight never went green" >&2
  gql 'query ($id: UUID!) { preflight(projectId: $id) { agentId ok detail } }' "{\"id\":\"$project\"}" | jq -c '.data.preflight' >&2
  tail -20 "$WORK/runner.log" >&2
  exit 1
fi

issue=$(gql 'mutation ($workspaceId: UUID!, $projectId: UUID!, $repoId: UUID!, $title: String!, $body: String!, $criteria: [String!]!) {
               createIssue(workspaceId: $workspaceId, projectId: $projectId, repoId: $repoId, title: $title, body: $body, criteria: $criteria) { id status }
             }' \
  "{\"workspaceId\":\"$workspace\",\"projectId\":\"$project\",\"repoId\":\"$repo\",\"title\":\"subtract two numbers\",\"body\":\"add a subtract function\",\"criteria\":[\"subtract(5, 3) returns 2\"]}" | jq -r '.data.createIssue.id // .errors')
echo "issue $issue"

gql 'mutation ($issueId: UUID!) { approveCriteria(issueId: $issueId) { status } }' "{\"issueId\":\"$issue\"}" | jq -c '.data.approveCriteria // .errors'
echo "ready"

start_run() {
  gql 'mutation ($issueId: UUID!, $role: String!) { startRun(issueId: $issueId, role: $role) { id status } }' \
    "{\"issueId\":\"$issue\",\"role\":\"$1\"}" | jq -c '.data.startRun // .errors'
}

start_run test_author
wait_status "$issue" "tests_in_progress" 60 || true
sleep 3
wait_status "$issue" "tests_in_progress" 30

# The base replay proves the authored tests fail at base; freezing before it lands
# is refused, so the driver waits for the verdict instead of guessing.
wait_replay() {
  local kind="$1" budget="${2:-240}"
  for _ in $(seq 1 "$budget"); do
    local row
    row=$(gql 'query ($id: UUID!) { issue(id: $id) { replays { kind status } } }' "{\"id\":\"$issue\"}" \
      | tee "$WORK/replay-poll.json" \
      | jq -r --arg kind "$kind" '[(.data.issue.replays // [])[] | select(.kind == $kind)] | last | "\(.kind // "-"):\(.status // "-")"')
    case "$row" in
      *:succeeded) echo "  replay $row"; return 0;;
      *:failed) echo "  replay $row" >&2; return 1;;
    esac
    sleep 1
  done
  echo "  ✗ replay $kind never finished" >&2
  return 1
}

echo "freeze"
wait_replay base "${SLOW_WAIT:-240}" || exit 1
gql 'mutation ($issueId: UUID!) { freezeSuite(issueId: $issueId) { status } }' "{\"issueId\":\"$issue\"}" | jq -c '.data.freezeSuite.status // .errors'

start_run implementer
wait_status "$issue" "in_review" "${SLOW_WAIT:-120}"

start_run reviewer
# The review is the gate: wait for the verdict rather than guessing how long a
# reviewer takes.
for _ in $(seq 1 "${SLOW_WAIT:-60}"); do
  verdicts=$(gql 'query ($id: UUID!) { issue(id: $id) { reviews { verdict } } }' "{\"id\":\"$issue\"}" | jq -r '.data.issue.reviews | length')
  [[ "$verdicts" != "0" ]] && break
  sleep 1
done
echo "  → review: $(gql 'query ($id: UUID!) { issue(id: $id) { reviews { verdict } } }' "{\"id\":\"$issue\"}" | jq -r '.data.issue.reviews[0].verdict')"

read -r run_id head sha flags <<<"$(gql 'query ($id: UUID!) { issue(id: $id) { currentRun { id headRevision diffSha flags } } }' "{\"id\":\"$issue\"}" \
  | jq -r '.data.issue.currentRun | "\(.id) \(.headRevision) \(.diffSha) \(.flags | join(","))"')"
echo "reviewed head ${head:0:12}, flags: ${flags:-(none)}"

echo "approve"
gql 'mutation ($issueId: UUID!, $runId: UUID!, $head: String!, $sha: String!, $flags: [String!]!) {
       approveIssue(issueId: $issueId, runId: $runId, reviewedHead: $head, diffSha: $sha, acknowledgeFlags: $flags) { status }
     }' "{\"issueId\":\"$issue\",\"runId\":\"$run_id\",\"head\":\"$head\",\"sha\":\"$sha\",\"flags\":[]}" \
  | jq -r '.data.approveIssue.status // .errors'
wait_status "$issue" "merging" "${SLOW_WAIT:-120}"

wait_replay landing "${SLOW_WAIT:-240}" || exit 1
landing=$(gql 'query ($id: UUID!) { issue(id: $id) { approval { landingBookmark } } }' "{\"id\":\"$issue\"}" | jq -r '.data.issue.approval.landingBookmark')
merge_revision=$( (cd "$REPO" && jj log -r "$landing" --no-graph -T commit_id) )
echo "merge $landing at ${merge_revision:0:12}"

gql 'mutation ($issueId: UUID!, $rev: String!) { recordMerge(issueId: $issueId, mergeRevision: $rev) { status } }' \
  "{\"issueId\":\"$issue\",\"rev\":\"$merge_revision\"}" | jq -c '.data.recordMerge.status // .errors'
wait_status "$issue" "done" "${SLOW_WAIT:-60}"

echo
echo "final:"
gql 'query ($id: UUID!) { issue(id: $id) { status attempt runs { role status } replays { kind status } reviews { verdict } } }' \
  "{\"id\":\"$issue\"}" | jq -c '.data.issue'
