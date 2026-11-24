#!/usr/bin/env bash
# A scripted agent for the end-to-end test. It is deliberately dumb: it proves the
# loop moves, and the assertions in the test prove the loop is honest.
set -euo pipefail

# A Lumi turn carries no agent name and no checkout: the question arrives as
# `--message`, and the answer is one JSON object on a plain line. The line is
# prefixed so the transcript keeps it as text rather than as an unknown event.
if [ -z "${HULAHOOP_AGENT:-}" ]; then
  message=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --message) message="${2:-}"; shift 2 ;;
      *) shift ;;
    esac
  done
  case "$message" in
    *follow-up*)
      echo 'LUMI {"reply":"One thing is missing: nothing says what subtract does below zero.","proposal":{"title":"File the follow-up","items":[{"group":"Tasks","text":"Create a task to decide what subtract does below zero","action":"create_issue","args":{"title":"decide what subtract does below zero","project":"Bali","body":"subtract(3, 5) has a decided answer"}}]}}'
      ;;
    *)
      echo 'LUMI {"reply":"The board looks healthy."}'
      ;;
  esac
  exit 0
fi

AGENT="${HULAHOOP_AGENT:?HULAHOOP_AGENT is not set}"
cd "$HULAHOOP_CHECKOUT"

echo "{\"type\":\"message_start\",\"role\":\"${AGENT}\"}"

# The document names the team: `ada` works and `bram` reviews. The script reads the
# agent's name because that is what the server hands it; there is no role to read.
case "$AGENT" in
  ada)
    # A question is a turn that ends with `HULAHOOP_QUESTION:` and nothing else.
    # The answer arrives as `blocked_answer` in the input of the turn that resumes
    # the work, so asking again after it is how the test learns the answer was lost.
    if jq -e '.issue.title | test("^ask me")' "$HULAHOOP_INPUT" >/dev/null 2>&1 &&
      jq -e '.blocked_answer == null' "$HULAHOOP_INPUT" >/dev/null 2>&1; then
      echo '{"type":"message","text":"This one needs a person."}'
      echo 'HULAHOOP_QUESTION: shall subtract saturate at zero instead of going negative?'
      exit 0
    fi
    # One commit does the whole job: the implementation and the test that proves
    # it. An agent owns both now, so there is no separate authoring stage.
    cat > tests/test_subtract.py <<'PY'


def test_ac1_subtract_returns_the_difference():
    from calc import subtract

    assert subtract(5, 3) == 2
PY
    printf '\n\ndef subtract(a: int, b: int) -> int:\n    return a - b\n' >>calc.py
    echo '{"type":"tool_call","name":"write","path":"calc.py"}'
    echo "HULAHOOP_VERDICT: approved"
    ;;
  bram)
    echo '{"type":"message","text":"One test fails before the change and passes after it."}'
    echo "HULAHOOP_VERDICT: approved"
    ;;
  *)
    echo "unknown agent ${AGENT}" >&2
    exit 1
    ;;
esac

echo "{\"type\":\"agent_settled\",\"aborted\":false}"
