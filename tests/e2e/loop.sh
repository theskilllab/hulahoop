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
    seen="$(gql 'query ($id: UUID!) { issue(id: $id) { status runs { role status error } } }' "{\"id\":\"$issue\"}" \
      | jq -r '.data.issue.status // "?"')"
    if [ "$seen" = "$wanted" ]; then
      return 0
    fi
    case "$seen" in
      blocked|done|cancelled)
        echo "the task stopped at $seen while waiting for $wanted" >&2
        gql 'query ($id: UUID!) { issue(id: $id) { runs { role status error } comments { body } } }' "{\"id\":\"$issue\"}" >&2
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
  local first
  first="$(head -1 "$WORK/agents")"
  echo "bindings: $(gql 'mutation ($p: UUID!, $b: [RoleBinding!]!) { setProjectAgents(projectId: $p, bindings: $b) { role agentId } }' \
    "{\"p\":\"$project\",\"b\":[{\"role\":\"worker\",\"agentId\":\"$first\"},{\"role\":\"reviewer\",\"agentId\":\"$first\"}]}" | jq -c '.data.setProjectAgents // .errors')"

  local issue
  issue="$(gql 'mutation ($w: UUID!, $p: UUID!, $r: UUID!) { createIssue(workspaceId: $w, projectId: $p, repoId: $r, title: "subtract two numbers", body: "add subtract", criteria: ["subtract(5, 3) returns 2"]) { id status } }' \
    "{\"w\":\"$workspace\",\"p\":\"$project\",\"r\":\"$repo\"}" | jq -r '.data.createIssue.id')"
  echo "issue $issue created"

  echo "planned: $(gql 'mutation ($id: UUID!) { planIssue(issueId: $id, planned: true) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.planIssue.status')"

  # No startRun here on purpose: moving the task to To do is the dispatch.
  wait_status "$issue" "in_review" "${SLOW_WAIT:-600}"
  echo "in review: $(gql 'query ($id: UUID!) { issue(id: $id) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.issue.status')"

  local head
  head="$(gql 'query ($id: UUID!) { issue(id: $id) { currentRun { headRevision } } }' "{\"id\":\"$issue\"}" \
    | jq -r '.data.issue.currentRun.headRevision // ""')"
  case "$head" in
    ????????????????????????????????????????) ;;
    *) echo "the review has no head revision: '$head'" >&2; return 1 ;;
  esac
  echo "head $head"

  gql 'mutation ($id: UUID!) { mergeIssue(issueId: $id) { status } }' "{\"id\":\"$issue\"}" | jq -c '.data.mergeIssue.status'
  wait_status "$issue" "done" 60
  echo "done: $(gql 'query ($id: UUID!) { issue(id: $id) { status runs { role status } } }' "{\"id\":\"$issue\"}" | jq -c '.data.issue')"
}

main "$@"
