#!/usr/bin/env bash
# A scripted agent for the end-to-end test. It is deliberately dumb: it proves the
# loop moves, and the assertions in the test prove the loop is honest.
set -euo pipefail

ROLE="${HULAHOOP_ROLE:?HULAHOOP_ROLE is not set}"
cd "$HULAHOOP_CHECKOUT"

echo "{\"type\":\"message_start\",\"role\":\"${ROLE}\"}"

case "$ROLE" in
  test_author)
    cat > tests/test_subtract.py <<'PY'


def test_ac1_subtract_returns_the_difference():
    # Imported inside the test: at the base revision calc has no subtract yet, and
    # a module-level import would turn the whole file into one collection error.
    from calc import subtract

    assert subtract(5, 3) == 2
PY
    echo "{\"type\":\"tool_call\",\"name\":\"write\",\"path\":\"tests/test_subtract.py\"}"
    ;;
  implementer)
    cat >> calc.py <<'PY'


def subtract(a: int, b: int) -> int:
    return a - b
PY
    echo "{\"type\":\"tool_call\",\"name\":\"edit\",\"path\":\"calc.py\"}"
    ;;
  reviewer)
    echo "{\"type\":\"message\",\"text\":\"The diff adds subtract and one failing-first test covers it.\"}"
    echo "HULAHOOP_VERDICT: approved"
    ;;
  *)
    echo "unknown role ${ROLE}" >&2
    exit 1
    ;;
esac

echo "{\"type\":\"agent_settled\",\"aborted\":false}"
