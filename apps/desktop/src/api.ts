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
  port: number;
  token: string;
  dataDir: string;
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

/** The Tauri shell owns the server, so the port and token come from it. */
export async function connect(): Promise<Connection> {
  if (connection) return connection;
  const bridge = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  if (bridge) {
    const { invoke } = await import("@tauri-apps/api/core");
    const info = await invoke<Connection>("connection");
    connection = info;
    return info;
  }
  const port = Number(localStorage.getItem("hulahoop.port") ?? 0);
  const token = localStorage.getItem("hulahoop.token") ?? "";
  if (!port || !token) {
    throw new Error(
      "No running server. Start the app (or `just dev`) and reload: the browser build reads the port and token from the connection file.",
    );
  }
  connection = { port, token, dataDir: "" };
  return connection;
}

export function rememberConnection(port: number, token: string) {
  localStorage.setItem("hulahoop.port", String(port));
  localStorage.setItem("hulahoop.token", token);
}

export class GqlError extends Error {}

export async function gql<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
  const { port, token } = await connect();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetch(`http://127.0.0.1:${port}/graphql`, {
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
  runs { id role status attempt adapter flags headBookmark headRevision diffSha error startedAt finishedAt }
  reviews { id reviewedRunId reviewerRunId verdict body createdAt }
  comments { id authorType body pinned createdAt }
  replays { id kind status failedTests createdAt }
  approval { id headRevision diffSha landingBookmark flags createdAt }
  currentRun { id role status attempt }
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
