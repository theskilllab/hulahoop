#!/usr/bin/env bash
# A scripted agent for the end-to-end test. It is deliberately dumb: it proves the
# loop moves, and the assertions in the test prove the loop is honest.
set -euo pipefail

ROLE="${HULAHOOP_ROLE:?HULAHOOP_ROLE is not set}"
cd "$HULAHOOP_CHECKOUT"

echo "{\"type\":\"message_start\",\"role\":\"${ROLE}\"}"

case "$ROLE" in
  worker)
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
  reviewer)
    echo '{"type":"message","text":"One test fails before the change and passes after it."}'
    echo "HULAHOOP_VERDICT: approved"
    ;;
  *)
    echo "unknown role ${ROLE}" >&2
    exit 1
    ;;
esac

echo "{\"type\":\"agent_settled\",\"aborted\":false}"
