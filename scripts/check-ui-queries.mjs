#!/usr/bin/env node
/**
 * The desktop sends GraphQL as strings, so the Rust compiler never sees them and
 * `pnpm build` cannot notice a field the server stopped exposing. This posts every
 * query and mutation found in the UI source to a running server and fails on the
 * errors that mean drift: unknown field, unknown argument, unknown type, syntax.
 *
 * It needs a server, because only a server knows its schema:
 *   HULAHOOP_SERVER=http://127.0.0.1:9000 HULAHOOP_TOKEN=… node scripts/check-ui-queries.mjs
 * Without HULAHOOP_SERVER it reads `connection.json` from HULAHOOP_DATA or the
 * platform data directory, which is what `just dev` leaves behind.
 *
 * Variables are sent empty: the server answers "missing variable" and that is fine,
 * because validation of the selection set happens before execution.
 */
import { readdir, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

const DOCUMENT = /gql<[^>]*>\(\s*`([^`]*)`/g;

async function files(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const found = [];
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) found.push(...(await files(path)));
    else if (entry.name.endsWith(".ts") || entry.name.endsWith(".tsx")) found.push(path);
  }
  return found;
}

function connection() {
  const url = process.env.HULAHOOP_SERVER;
  const token = process.env.HULAHOOP_TOKEN;
  if (url && token) return { url, token };
  const data =
    process.env.HULAHOOP_DATA ??
    (process.platform === "darwin"
      ? join(homedir(), "Library", "Application Support", "hulahoop")
      : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "hulahoop"));
  return { data };
}

async function resolve() {
  const found = connection();
  if (found.url) return found;
  const path = join(found.data, "connection.json");
  let parsed;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch {
    throw new Error(`no server to ask: set HULAHOOP_SERVER and HULAHOOP_TOKEN, or start one so ${path} exists`);
  }
  return { url: parsed.url, token: parsed.token };
}

const DRIFT = /Unknown field|Unknown argument|Unknown type|Unknown directive|Syntax Error|expected one of/i;

const { url, token } = await resolve();
const sources = await files("apps/desktop/src");
const documents = new Map();
for (const source of sources) {
  const text = await readFile(source, "utf8");
  for (const match of text.matchAll(DOCUMENT)) {
    const document = match[1].replace(/\\n/g, "\n").trim();
    if (document && !documents.has(document)) documents.set(document, source);
  }
}

let failed = 0;
for (const [document, source] of documents) {
  const response = await fetch(`${url}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ query: document, variables: {} }),
  });
  const payload = await response.json();
  const drift = (payload.errors ?? []).filter((problem) => DRIFT.test(problem.message ?? ""));
  if (drift.length > 0) {
    failed += 1;
    const first = document.split("\n")[0].slice(0, 90);
    console.log(`✗ ${source}\n  ${first}`);
    for (const problem of drift) console.log(`  ${problem.message}`);
  }
}

console.log(`${documents.size} documents from ${sources.length} files, ${failed} with drift`);
process.exit(failed === 0 ? 0 : 1);
