export type IssueStatus =
  | "backlog"
  | "ready"
  | "tests_in_progress"
  | "tests_ready"
  | "in_progress"
  | "in_review"
  | "merging"
  | "blocked"
  | "done"
  | "cancelled";

export type Role = "test_author" | "implementer" | "reviewer";

export interface Connection {
  /** The server's GraphQL endpoint root, like `http://127.0.0.1:51234`. */
  url: string;
  token: string;
  dataDir: string;
  /** What to show in the rail: a host name, or `local`. */
  label: string;
}

export interface Criterion {
  position: number;
  text: string;
  testName: string | null;
}

export interface Run {
  id: string;
  role: Role;
  status: string;
  attempt: number;
  adapter: string;
  flags: string[];
  headBookmark: string | null;
  headRevision: string | null;
  diffSha: string | null;
  commitName: string | null;
  commitEmail: string | null;
  prUrl: string | null;
  prNumber: number | null;
  error: string | null;
  startedAt: string | null;
  finishedAt: string | null;
}

export interface Review {
  id: string;
  reviewedRunId: string;
  reviewerRunId: string;
  verdict: string;
  body: string;
  createdAt: string;
}

export interface Comment {
  id: string;
  authorType: string;
  body: string;
  pinned: boolean;
  createdAt: string;
}

export interface Replay {
  id: string;
  kind: "base" | "head" | "landing";
  status: string;
  failedTests: string[] | null;
  createdAt: string;
}

export interface Approval {
  id: string;
  headRevision: string;
  diffSha: string;
  landingBookmark: string | null;
  flags: string[];
  createdAt: string;
}

export interface Issue {
  id: string;
  title: string;
  body: string;
  status: IssueStatus;
  priority: string;
  attempt: number;
  runCount: number;
  testsFrozenAt: string | null;
  mergeRevision: string | null;
  pinnedCommentId: string | null;
  criteria: Criterion[];
  runs: Run[];
  reviews: Review[];
  comments: Comment[];
  replays: Replay[];
  approval: Approval | null;
  currentRun: Run | null;
}

export interface Repo {
  id: string;
  name: string;
  path: string;
  vcs: string;
  baseBookmark: string;
  contractVersion: string;
  fetchedAt: string | null;
}

export interface Agent {
  id: string;
  name: string;
  adapter: string;
  model: string;
}

export interface ProjectAgent {
  projectId: string;
  agentId: string;
  role: Role;
}

export interface Project {
  id: string;
  name: string;
  repos: Repo[];
}

export interface Workspace {
  id: string;
  name: string;
}

let connection: Connection | null = null;

const REMOTE_KEY = "hulahoop.remote";

interface SavedRemote {
  url: string;
  token: string;
}

/** A server saved from the connect screen; work runs wherever it listens. */
export function savedRemote(): SavedRemote | null {
  const raw = localStorage.getItem(REMOTE_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as SavedRemote;
    return parsed.url && parsed.token ? parsed : null;
  } catch {
    return null;
  }
}

export function saveRemote(url: string, token: string) {
  localStorage.setItem(
    REMOTE_KEY,
    JSON.stringify({ url: normalizeServer(url), token: token.trim() }),
  );
  connection = null;
}

/** Forget the saved host and fall back to the server this machine runs. */
export function useLocalServer() {
  localStorage.removeItem(REMOTE_KEY);
  connection = null;
}

function labelFor(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/**
 * Where the work runs. A saved host wins, because that is the one the person
 * chose; otherwise the Tauri shell's own server answers, and a browser dev build
 * falls back to the port and token pasted from a connection file.
 */
export async function connect(): Promise<Connection> {
  if (connection) return connection;

  const saved = savedRemote();
  if (saved) {
    connection = {
      url: saved.url,
      token: saved.token,
      dataDir: "",
      label: labelFor(saved.url),
    };
    return connection;
  }

  const bridge = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  if (bridge) {
    const { invoke } = await import("@tauri-apps/api/core");
    const info = await invoke<Connection>("connection");
    connection = info;
    return info;
  }

  const url = localStorage.getItem("hulahoop.url") ?? "";
  const port = Number(localStorage.getItem("hulahoop.port") ?? 0);
  const token = localStorage.getItem("hulahoop.token") ?? "";
  if ((!url && !port) || !token) {
    throw new Error(
      "No server yet. Start one on any machine and connect to it: `hulahoop-server --listen 0.0.0.0 --port 5123` prints the token with --print-connection.",
    );
  }
  const base = url || `http://127.0.0.1:${port}`;
  connection = { url: base, token, dataDir: "", label: labelFor(base) };
  return connection;
}

export function rememberConnection(url: string, token: string) {
  localStorage.setItem("hulahoop.url", url.replace(/\/+$/, ""));
  localStorage.setItem("hulahoop.port", String(Number(new URL(url).port || 0)));
  localStorage.setItem("hulahoop.token", token);
}

/** One cheap query, so a wrong host or token fails at the connect screen. */
/**
 * What people type in the connect screen: `192.168.1.19:9000`, `http://host:9000/`,
 * or a full URL. Without a scheme, `fetch("192.168.1.19:9000/graphql")` is parsed as
 * the custom scheme `192.168.1.19`, and WebKit reports that as "The string did not
 * match the expected pattern" — an error that says nothing about the real problem.
 */
export function normalizeServer(input: string): string {
  const trimmed = input.trim().replace(/\/+$/, "");
  if (!trimmed) {
    throw new Error("enter the server address, for example 192.168.1.19:9000");
  }
  const withScheme = trimmed.includes("://") ? trimmed : `http://${trimmed}`;
  let parsed: URL;
  try {
    parsed = new URL(withScheme);
  } catch {
    throw new Error(`"${input}" is not a server address; try 192.168.1.19:9000`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new Error(`use http:// or https://, not ${parsed.protocol}`);
  }
  if (!parsed.hostname) {
    throw new Error(`"${input}" has no host; try 192.168.1.19:9000`);
  }
  return `${parsed.protocol}//${parsed.host}`;
}

export async function probe(server: string, token: string): Promise<void> {
  const url = normalizeServer(server);
  const trimmed = token.trim();
  if (!trimmed) {
    throw new Error("enter the token the server printed");
  }
  let response: Response;
  try {
    response = await fetch(`${url}/graphql`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${trimmed}` },
      body: JSON.stringify({ query: "{ workspaces { id name } }" }),
    });
  } catch {
    throw new Error(`could not reach ${url}. Is the server running, and does it listen on that address?`);
  }
  if (!response.ok) {
    throw new Error(`${url} answered ${response.status}; check the token`);
  }
  const payload = await response.json();
  if (payload.errors?.length) {
    throw new Error(payload.errors.map((e: { message: string }) => e.message).join("\n"));
  }
}

export class GqlError extends Error {}

export async function gql<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
  const { url, token } = await connect();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetch(`${url}/graphql`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ query, variables }),
      signal: controller.signal,
    });
    const payload = await response.json();
    if (payload.errors?.length) {
      throw new GqlError(payload.errors.map((e: { message: string }) => e.message).join("\n"));
    }
    return payload.data as T;
  } finally {
    clearTimeout(timeout);
  }
}

export const ISSUE_FIELDS = `
  id title body status priority attempt runCount testsFrozenAt mergeRevision pinnedCommentId
  criteria { position text testName }
  runs { id role status attempt adapter flags headBookmark headRevision diffSha commitName commitEmail prUrl prNumber error startedAt finishedAt }
  reviews { id reviewedRunId reviewerRunId verdict body createdAt }
  comments { id authorType body pinned createdAt }
  replays { id kind status failedTests createdAt }
  approval { id headRevision diffSha landingBookmark flags createdAt }
  currentRun { id role status attempt adapter flags headBookmark headRevision diffSha commitName commitEmail prUrl prNumber error startedAt finishedAt }
`;

export async function loadBoard(projectId: string): Promise<Issue[]> {
  const data = await gql<{ issues: Issue[] }>(
    `query ($projectId: UUID!) { issues(projectId: $projectId) { ${ISSUE_FIELDS} } }`,
    { projectId },
  );
  return data.issues;
}

export async function loadIssue(id: string): Promise<Issue> {
  const data = await gql<{ issue: Issue }>(`query ($id: UUID!) { issue(id: $id) { ${ISSUE_FIELDS} } }`, { id });
  return data.issue;
}

export async function loadWorkspace(): Promise<{ workspace: Workspace; project: Project | null; agents: Agent[] }> {
  const data = await gql<{ workspaces: Workspace[] }>(`{ workspaces { id name } }`);
  const workspace = data.workspaces[0];
  if (!workspace) throw new Error("the server has no workspace yet");
  const projects = await gql<{ projects: Project[] }>(
    `query ($id: UUID!) { projects(workspaceId: $id) { id name repos { id name path vcs baseBookmark contractVersion fetchedAt } } }`,
    { id: workspace.id },
  );
  const agents = await gql<{ agents: Agent[] }>(
    `query ($id: UUID!) { agents(workspaceId: $id) { id name adapter model } }`,
    { id: workspace.id },
  );
  return { workspace, project: projects.projects[0] ?? null, agents: agents.agents };
}

export async function loadWorkspaceIssues(workspaceId: string): Promise<Issue[]> {
  const data = await gql<{ projects: Project[] }>(
    `query ($id: UUID!) { projects(workspaceId: $id) { id name repos { id name path vcs baseBookmark contractVersion fetchedAt } } }`,
    { id: workspaceId },
  );
  const project = data.projects[0];
  if (!project) return [];
  return loadBoard(project.id);
}
