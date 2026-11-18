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
test:
    mkdir -p .hulahoop target/nextest/ci
    cargo nextest run --workspace --profile ci
    cargo test --workspace --doc
    cp target/nextest/ci/junit.xml .hulahoop/junit.xml

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

# What CI runs.
ci: fmt-check lint test
    pnpm --prefix apps/desktop build

# Start the server and the runner against a scratch data directory.
dev:
    ./scripts/dev.sh

# Attach a repository and create the three agents.
seed path:
    cargo run -q -p hulahoop-cli -- seed --repo {{path}}

# Build the unsigned macOS bundle for a Rust target triple.
bundle target:
    ./scripts/bundle.sh {{target}}

# Why can't the app reach the server? Steps through address, port, /health, token.
debug target="127.0.0.1:9000" port="":
    ./scripts/debug.sh {{target}} {{port}}

# Pack hulahoop-server, hulahoop-runner, and the CLI. No argument means this host.
headless target="":
    ./scripts/headless.sh {{target}}

# Redraw the app icon (PNG, icns, ico) from scripts/make-icons.py.
icons:
    python3 scripts/make-icons.py
