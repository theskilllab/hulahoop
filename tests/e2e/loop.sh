#!/usr/bin/env bash
# Runs one task through the whole loop against a throwaway repo with the fake
# adapter: backlog → to do → an agent takes it → review → merge → done.
#
# The point is the plumbing, not the model: a task in To do has to come back in
# review with a commit and a pull request, and the merge has to close it.
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT="$PWD"
WORK="$(mktemp -d /tmp/hulahoop-e2e.XXXXXX)"
REPO="$WORK/repo"
DATA="$WORK/data"
mkdir -p "$REPO" "$DATA"

echo "work dir: $WORK"

CONN="$DATA/connection.json"
# Reads the server's own tables. The dev shell carries sqlite3 and prints a banner,
# so the banner is dropped rather than shown inside a value.
sql() {
  if command -v sqlite3 >/dev/null 2>&1; then
    sqlite3 "$DATA/hulahoop.sqlite" "$1"
  else
    nix develop --command sqlite3 "$DATA/hulahoop.sqlite" "$1" 2>/dev/null \
      | sed '/^hulahoop dev shell:/d'
  fi
}
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
  ( cd "$REPO" && jj git init --colocate >/dev/null 2>&1 || true )
  ( cd "$REPO" && jj bookmark create main -r @- 2>/dev/null || jj bookmark set main -r @- )
}

wait_status() {
  local issue="$1" wanted="$2" budget="${3:-180}" seen=""
  for _ in $(seq 1 "$budget"); do
    seen="$(gql 'query ($id: UUID!) { issue(id: $id) { status runs { status error } } }' "{\"id\":\"$issue\"}" \
      | jq -r '.data.issue.status // "?"')"
    if [ "$seen" = "$wanted" ]; then
      return 0
    fi
    case "$seen" in
      blocked|done|cancelled)
        echo "the task stopped at $seen while waiting for $wanted" >&2
        gql 'query ($id: UUID!) { issue(id: $id) { runs { status error } comments(newestFirst: false) { body } } }' "{\"id\":\"$issue\"}" >&2
        return 1
        ;;
    esac
    sleep 1
  done
  echo "the task stayed at $seen for ${budget}s while waiting for $wanted" >&2
  return 1
}

cleanup() {
  local code=$?
  [ -n "${RUNNER_PID:-}" ] && kill "$RUNNER_PID" 2>/dev/null || true
  [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null || true
  if [ "$code" -ne 0 ]; then
    echo "kept $WORK for inspection" >&2
  else
    rm -rf "$WORK"
  fi
  exit "$code"
}
trap cleanup EXIT

setup() {
  copy_fixture
  nix develop --command cargo build -q -p hulahoop-cli 2>/dev/null || cargo build -q -p hulahoop-cli
  HULAHOOP_DATA="$DATA" "$ROOT/target/debug/hulahoop" serve --data-dir "$DATA" >"$WORK/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 60); do [ -f "$CONN" ] && break; sleep 0.5; done
  [ -f "$CONN" ] || { echo "the server never wrote $CONN" >&2; cat "$WORK/server.log" >&2; return 1; }

  HULAHOOP_FAKE_SCRIPT="$ROOT/tests/e2e/fake_agent.sh" \
  HULAHOOP_RUNNER_MODE=local \
    "$ROOT/target/debug/hulahoop" runner --server "http://127.0.0.1:$(port)" --token "$(token)" --data-dir "$DATA" \
    >"$WORK/runner.log" 2>&1 &
  RUNNER_PID=$!
}

main() {
  setup

  local workspace project repo
  workspace="$(gql 'mutation { createWorkspace(name: "e2e") { id } }' '{}' | jq -r '.data.createWorkspace.id')"
  project="$(gql 'mutation ($w: UUID!) { createProject(workspaceId: $w, name: "Bali") { id } }' "{\"w\":\"$workspace\"}" \
    | jq -r '.data.createProject.id')"
  repo="$(gql 'mutation ($p: UUID!, $path: String!) { addRepo(projectId: $p, name: "repo", path: $path) { id } }' \
    "{\"p\":\"$project\",\"path\":\"$REPO\"}" | jq -r '.data.addRepo.id')"

  local agents="" name id
  for name in ada bram; do
    id="$(gql 'mutation ($w: UUID!, $n: String!) { createAgent(workspaceId: $w, name: $n, adapter: "fake", model: "fake") { id } }' "{\"w\":\"$workspace\",\"n\":\"$name\"}" \
      | jq -r '.data.createAgent.id')"
    echo "probe: $(gql 'mutation ($a: UUID!, $r: UUID!) { runPreflight(agentId: $a, repoId: $r) }' \
      "{\"a\":\"$id\",\"r\":\"$repo\"}" | jq -c '.data.runPreflight // .errors')"
    for _ in $(seq 1 600); do
      ok="$(gql 'query ($p: UUID!) { preflight(projectId: $p) { agentId ok detail } }' "{\"p\":\"$project\"}" \
        | jq -r --arg a "$id" '(.data.preflight // [])[] | select(.agentId == $a) | .ok' | head -1)"
      [ "$ok" = "true" ] && break
      [ "$ok" = "false" ] && { echo "preflight for $name failed" >&2; return 1; }
      sleep 1
    done
    [ "$ok" = "true" ] || { echo "preflight for $name never finished" >&2; return 1; }
    echo "$id" >>"$WORK/agents"
    echo "agent $name -> $id"
  done
  # The document names the team: a status that names an agent has nobody to
  # fall back to now, because there is no second place that maps jobs to people.
  local doc
  doc=$(cat <<'DOC'
# Workflows

## Backlog

Users: human
Next: To do

Work you still want to do yourself. Nothing here is dispatched.

## To do

Users: ada
Next: In progress

Ready for the team. The agent picks a task up from here.

## In progress

Users: bram
Next: In review - when the work is ready to read
Next: Blocked - when the agent needs a decision it cannot make itself

The agent works against the task. The reviewer reads the work and sends it
back when it is not there yet.

## Blocked

Users: human
Next: In progress - when the question is answered

The agent asked a question, or the work waits on somebody outside.

## In review

Users: human
Next: Done - when the merge lands

Somebody reads the work and merges it.

## Done

Users: ada

The work landed. This document names an agent even here, which is the shape that
used to start a turn on a finished task: nothing is dispatched from a finished
column, whatever it names.
DOC
)
  echo "workflow: $(gql 'mutation ($w: UUID!, $s: String!) { setWorkflow(workspaceId: $w, source: $s) { errors } }' \
    "$(jq -cn --arg w "$workspace" --arg s "$doc" '{w:$w,s:$s}')" | jq -c '.data.setWorkflow // .errors')"

  # A question is the workflow's other exit: the agent asks, the task lands in
  # Blocked the way the document's `Next: Blocked` line says, and the answer puts
  # it back where it was with the pair in the next turn's input. The fake agent
  # asks only when that input carries no answer, so a lost answer shows up as a
  # second question rather than as a passing test.
  local asked question
  asked="$(gql 'mutation ($w: UUID!, $p: UUID!, $r: UUID!) { createIssue(workspaceId: $w, projectId: $p, repoId: $r, title: "ask me: does subtract saturate?", body: "a question for a person") { id } }' \
    "{\"w\":\"$workspace\",\"p\":\"$project\",\"r\":\"$repo\"}" | jq -r '.data.createIssue.id')"
  echo "question issue $asked created"
  echo "planned: $(gql 'mutation ($id: UUID!) { planIssue(issueId: $id, planned: true) { status } }' "{\"id\":\"$asked\"}" | jq -c '.data.planIssue.status')"
  wait_status "$asked" "blocked" 240
  moves="$(gql 'query ($id: UUID!) { issue(id: $id) { moves } }' "{\"id\":\"$asked\"}" | jq -c '.data.issue.moves')"
  case "$moves" in
    *in_progress*) echo "blocked exits: $moves" ;;
    *) echo "the document names no exit from blocked: '$moves'" >&2; return 1 ;;
  esac
  question="$(gql 'query ($id: UUID!) { issue(id: $id) { comments(newestFirst: false) { body pinned } } }' "{\"id\":\"$asked\"}" \
    | jq -r '[.data.issue.comments[] | select(.pinned) | .body] | last // ""')"
  case "$question" in
    *saturate*) echo "question: $question" ;;
    *) echo "the pinned comment is not the question: '$question'" >&2; return 1 ;;
  esac
  echo "answered: $(gql 'mutation ($id: UUID!, $a: String!) { resumeIssue(issueId: $id, answer: $a) { status } }' \
    "{\"id\":\"$asked\",\"a\":\"no, it may go negative\"}" | jq -c '.data.resumeIssue.status')"
  wait_status "$asked" "in_review" 240
  echo "after the answer: $(gql 'query ($id: UUID!) { issue(id: $id) { status runs { status } } }' "{\"id\":\"$asked\"}" | jq -c '.data.issue')"

  local issue
  issue="$(gql 'mutation ($w: UUID!, $p: UUID!, $r: UUID!) { createIssue(workspaceId: $w, projectId: $p, repoId: $r, title: "subtract two numbers", body: "add subtract") { id status } }' \
    "{\"w\":\"$workspace\",\"p\":\"$project\",\"r\":\"$repo\"}" | jq -r '.data.createIssue.id')"
  echo "issue $issue created"

  echo "planned: $(gql 'mutation ($id: UUID!) { planIssue(issueId: $id, planned: true) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.planIssue.status')"

  # No startRun here on purpose: moving the task to To do is the dispatch.
  wait_status "$issue" "in_review" "${SLOW_WAIT:-600}"
  echo "in review: $(gql 'query ($id: UUID!) { issue(id: $id) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.issue.status')"

  # The review turn leaves the head alone: the work before it is what a merge
  # takes, so the newest published head is read from the runs, not from currentRun.
  local head
  head="$(gql 'query ($id: UUID!) { issue(id: $id) { runs { headRevision } } }' "{\"id\":\"$issue\"}" \
    | jq -r '[.data.issue.runs[] | .headRevision | select(. != null)] | last // ""')"
  case "$head" in
    ????????????????????????????????????????) ;;
    *) echo "the review has no head revision: '$head'" >&2; return 1 ;;
  esac
  echo "head $head"

  # The pull request names a turn that published a head. A reviewer publishes
  # none, and naming one asks the runner to push a branch that does not exist.
  local pr_run pr_head
  pr_run="$(sql "select json_extract(payload, '$.run_id') from runner_jobs where kind = 'open_pr' order by created_at desc limit 1;")"
  case "${#pr_run}" in
    36) ;;
    *) echo "no pull request was asked for: '$pr_run'" >&2; return 1 ;;
  esac
  pr_head="$(sql "select coalesce(head_bookmark, '') from runs where lower(hex(id)) = lower(replace('$pr_run', '-', ''));")"
  case "$pr_head" in
    ?*) echo "pull request from run $pr_run at $pr_head" ;;
    *) echo "the pull request names a turn with no head: '$pr_run'" >&2; return 1 ;;
  esac

  gql 'mutation ($id: UUID!) { mergeIssue(issueId: $id) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.mergeIssue.status'
  wait_status "$issue" "done" 60
  # A finished task starts nothing, whatever its section names: a merge that
  # dispatched a turn would run an agent against work nobody can advance.
  local settled before
  settled="$(gql 'query ($id: UUID!) { issue(id: $id) { runs { status } } }' "{\"id\":\"$issue\"}" | jq -c '[.data.issue.runs[].status]')"
  sleep 6
  before="$settled"
  settled="$(gql 'query ($id: UUID!) { issue(id: $id) { runs { status } } }' "{\"id\":\"$issue\"}" | jq -c '[.data.issue.runs[].status]')"
  if [ "$before" != "$settled" ]; then
    echo "a finished task started another run: $before then $settled" >&2
    return 1
  fi
  echo "done: $(gql 'query ($id: UUID!) { issue(id: $id) { status runs { status } reviews { verdict } } }' "{\"id\":\"$issue\"}" | jq -c '.data.issue')"

  # Lumi: a question goes through the runner like any job, the answer may carry
  # a proposal, and Apply is the only thing that changes the board.
  echo "lumi: $(gql 'mutation ($w: UUID!, $h: String!, $m: String!) { setWorkspaceLumi(workspaceId: $w, harness: $h, model: $m) { lumiHarness lumiModel } }' \
    "{\"w\":\"$workspace\",\"h\":\"fake\",\"m\":\"fake\"}" | jq -c '.data.setWorkspaceLumi // .errors')"
  echo "asked: $(gql 'mutation ($w: UUID!, $t: String!, $c: String) { askLumi(workspaceId: $w, text: $t, context: $c) { role } }' \
    "{\"w\":\"$workspace\",\"t\":\"file a follow-up task for subtract\",\"c\":\"the board\"}" | jq -c '.data.askLumi // .errors')"
  local answer=""
  for _ in $(seq 1 120); do
    answer="$(gql 'query ($w: UUID!) { lumiMessages(workspaceId: $w) { role body proposal proposalState } }' "{\"w\":\"$workspace\"}" \
      | jq -c '[.data.lumiMessages[] | select(.role == "lumi")] | last // empty')"
    [ -n "$answer" ] && break
    sleep 1
  done
  case "$answer" in
    *"nothing says what subtract does"*) echo "lumi answered" ;;
    *) echo "lumi never answered: '$answer'" >&2; return 1 ;;
  esac
  case "$answer" in
    *below\ zero*) ;;
    *) echo "the answer carries no proposal: '$answer'" >&2; return 1 ;;
  esac
  local lumi_msg
  lumi_msg="$(gql 'query ($w: UUID!) { lumiMessages(workspaceId: $w) { id role proposalState } }' "{\"w\":\"$workspace\"}" \
    | jq -r '[.data.lumiMessages[] | select(.role == "lumi" and .proposalState == "open")] | last | .id')"
  echo "applied: $(gql 'mutation ($id: UUID!) { applyLumiProposal(messageId: $id, items: [0]) { proposalState } }' \
    "{\"id\":\"$lumi_msg\"}" | jq -c '.data.applyLumiProposal // .errors')"
  local filed
  filed="$(gql 'query ($w: UUID!) { issues(workspaceId: $w) { title status } }' "{\"w\":\"$workspace\"}" \
    | jq -r '[.data.issues[] | select(.title | test("below zero"))] | length')"
  case "$filed" in
    1) echo "lumi filed the follow-up" ;;
    *) echo "apply did not create the task (found $filed)" >&2; return 1 ;;
  esac
}

main "$@"
