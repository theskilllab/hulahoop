#!/usr/bin/env bash
# Answers "why can't the app reach the server" without a rebuild.
#
#   just debug 192.168.1.19 9000
#   just debug 192.168.1.19:9000
#   HULAHOOP_TOKEN=… just debug
#
# It checks, in order: the address it was given, whether the port is open, whether
# the server answers /health, and whether the token gets through GraphQL. The first
# failing step is the answer, and each one prints what to do next.
set -uo pipefail

TARGET="${1:-127.0.0.1:9000}"
PORT_ARG="${2:-}"

if [[ "$TARGET" == *:* ]]; then
  HOST="${TARGET%:*}"
  PORT="${TARGET##*:}"
elif [[ -n "$PORT_ARG" ]]; then
  HOST="$TARGET"
  PORT="$PORT_ARG"
else
  HOST="$TARGET"
  PORT=9000
fi
HOST="${HOST#http://}"
HOST="${HOST#https://}"
HOST="${HOST%/}"
BASE="http://$HOST:$PORT"

fail() {
  printf '\n%s\n' "FAIL: $1"
  shift
  for line in "$@"; do printf '  %s\n' "$line"; done
  exit 1
}

ok() { printf '  ok   %s\n' "$1"; }
info() { printf '  ..   %s\n' "$1"; }

printf 'target %s (host %s, port %s)\n' "$BASE" "$HOST" "$PORT"

# 1. Where is the server actually bound? Only meaningful for this machine, and it
#    is the difference between "reachable from the LAN" and "loopback only".
if [[ "$HOST" == "127.0.0.1" || "$HOST" == "localhost" || "$HOST" == "$(hostname 2>/dev/null)" ||
      "$HOST" == "$(hostname -I 2>/dev/null | awk '{print $1}')" ]]; then
  if command -v ss >/dev/null; then
    bound=$(ss -ltn 2>/dev/null | awk -v p=":$PORT" '$4 ~ p {print $4}' | head -1)
    if [[ -z "$bound" ]]; then
      data="${HULAHOOP_DATA:-$HOME/.local/share/hulahoop}"
      file="$data/connection.json"
      # -x on the process name: matching the command line also matches this script,
      # which mentions the binaries in its own arguments.
      processes=$(pgrep -ax hulahoop-server 2>/dev/null | head -3)
      if [[ -n "$processes" ]]; then
        info "a hulahoop process is running, but on another port"
        printf '       %s\n' "$processes"
      elif [[ -f "$file" ]]; then
        info "$file is stale: nothing holds the port, but the file is still here"
        printf '       %s\n' "the last server was killed before it could clean up"
      else
        info "no hulahoop-server process and no $file"
        printf '       %s\n' "the server exited, and with it the token it minted"
      fi
      fail "nothing is listening on port $PORT on this machine" \
        "start it, and leave it running: hulahoop up --listen 0.0.0.0 --port $PORT" \
        "the token is printed by that process: --print-connection, or read" \
        "  $file once it is up (mode 0600)"
    elif [[ "$bound" == 127.0.0.1:* ]]; then
      info "listening on $bound — loopback only, so another machine cannot reach it"
      printf '  !!   %s\n' "restart with --listen 0.0.0.0 to accept other machines"
    else
      ok "listening on $bound"
    fi
  fi
fi

# 2. Is the port open from here?
if timeout 5 bash -c "exec 3<>/dev/tcp/$HOST/$PORT" 2>/dev/null; then
  ok "tcp $HOST:$PORT is open"
else
  fail "cannot open a tcp connection to $HOST:$PORT" \
    "if the server runs here: --listen 0.0.0.0, not the default 127.0.0.1" \
    "if it runs there: firewall. On NixOS, networking.firewall.allowedTCPPorts = [ $PORT ];" \
    "  check with: nmap -p $PORT $HOST   (or: nc -vz $HOST $PORT)" \
    "if you are on a network that blocks peer traffic, use a tunnel:" \
    "  ssh -L $PORT:localhost:$PORT $(whoami)@$HOST"
fi

# 3. Does the HTTP layer answer?
health=$(curl -sS -m 5 -o - -w '%{http_code}' "$BASE/health" 2>&1)
code="${health: -3}"
body="${health%???}"
if [[ "$code" == "200" && "$body" == "ok" ]]; then
  ok "/health answers ok"
else
  fail "/health answered $code" "the port is open but this is not a hulahoop server" \
    "check the port, and that hulahoop-server is the process holding it"
fi

# 4. Does the token get through?
TOKEN="${HULAHOOP_TOKEN:-}"
source="HULAHOOP_TOKEN"
if [[ -z "$TOKEN" ]]; then
  DATA="${HULAHOOP_DATA:-$HOME/.local/share/hulahoop}"
  # `just dev` runs against a scratch dir beside the code, so look there too.
  [[ -f "$DATA/connection.json" ]] || DATA="$PWD/.hulahoop-dev"
  [[ -f "$DATA/connection.json" ]] || DATA="$HOME/Library/Application Support/hulahoop"
  if [[ -f "$DATA/connection.json" ]]; then
    TOKEN=$(jq -r '.token // empty' "$DATA/connection.json" 2>/dev/null)
    source="$DATA/connection.json"
  fi
fi
if [[ -z "$TOKEN" ]]; then
  info "no token found (no HULAHOOP_TOKEN, no connection.json here)"
  printf '  !!   %s\n' "this is the machine the app runs on; read the token where the server runs:"
  printf '         jq -r .token ~/.local/share/hulahoop/connection.json\n'
  printf '       then: HULAHOOP_TOKEN=<token> just debug %s %s\n' "$HOST" "$PORT"
  exit 1
fi
ok "token from $source"

reply=$(curl -sS -m 10 -X POST "$BASE/graphql" \
  -H 'content-type: application/json' \
  -H "authorization: Bearer $TOKEN" \
  -d '{"query":"{ workspaces { id name } }"}' 2>&1)
if grep -q '"errors"' <<<"$reply"; then
  fail "GraphQL answered with errors" "$reply" \
    "the token may have changed: the server mints a new one every start"
else
  ok "graphql: $(jq -c '.data.workspaces[0].name // "?"' <<<"$reply" 2>/dev/null || echo "answered")"
  printf '  ..   answer: %s\n' "$(jq -c '.data' <<<"$reply" 2>/dev/null)"
fi

printf '\nall checks passed for %s\n' "$BASE"
printf 'in the app: connect screen → server %s, then the token above\n' "$BASE"
