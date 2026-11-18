set shell := ["bash", "-euo", "pipefail", "-c"]

# Recipes are the contract: `setup` prepares, `test` proves. Everything else is a
# convenience for humans and CI.

default:
    @just --list

# Install dependencies. The only recipe allowed to touch the network.
setup:
    cargo fetch
    pnpm --prefix apps/desktop install --frozen-lockfile

# Run the whole suite and write the machine-readable verdict.
# The JUnit report is the verdict every gate reads, so it has to exist after a red run
# too. A red suite is the wanted outcome of the base replay, and a recipe that copies the
# report only after a green run leaves that replay with no evidence to read.
test:
    #!/usr/bin/env bash
    set -uo pipefail
    mkdir -p .hulahoop target/nextest/ci
    suite=0
    cargo nextest run --workspace --profile ci || suite=$?
    docs=0
    cargo test --workspace --doc || docs=$?
    cp -f target/nextest/ci/junit.xml .hulahoop/junit.xml
    exit $(( suite != 0 ? suite : docs ))

# Check formatting without changing files.
fmt-check:
    cargo fmt --all --check
    pnpm --prefix apps/desktop exec tsc --noEmit

# Format the workspace.
fmt:
    cargo fmt --all

# Compile the Tauri shell. CI runs this on Linux so a shell bug fails fast.
shell-check:
    bash scripts/shell-check.sh

# Lint with warnings as errors.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# The desktop sends GraphQL as strings, so nothing compiles them: this posts every
# document to a throwaway server and fails on a field the schema dropped.
check-ui:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -q --bin hulahoop
    data="$(mktemp -d)"
    ./target/debug/hulahoop serve --data-dir "$data" --port 9123 >"$data/server.log" 2>&1 &
    server=$!
    trap 'kill "$server" 2>/dev/null || true' EXIT
    for _ in $(seq 1 60); do [ -f "$data/connection.json" ] && break; sleep 0.5; done
    HULAHOOP_SERVER=http://127.0.0.1:9123 \
      HULAHOOP_TOKEN="$(jq -r .token "$data/connection.json")" \
      node scripts/check-ui-queries.mjs

# What CI runs.
ci: fmt-check lint test check-ui
    pnpm --prefix apps/desktop build

# Start the server and the runner against a scratch data directory.
dev host="127.0.0.1" port="9000":
    ./scripts/dev.sh {{host}} {{port}}

# Attach a repository and create the three agents.
seed path:
    cargo run -q -p hulahoop-cli -- seed --repo {{path}}

# Build the unsigned macOS bundle for a Rust target triple.
bundle target:
    ./scripts/bundle.sh {{target}}

# Why can't the app reach the server? Steps through address, port, /health, token.
debug target="127.0.0.1:9000" port="":
    ./scripts/debug.sh {{target}} {{port}}

# Pack the hulahoop binary. No argument means this host.
headless target="":
    ./scripts/headless.sh {{target}}

# Redraw the app icon (PNG, icns, ico) from scripts/make-icons.py.
icons:
    python3 scripts/make-icons.py
