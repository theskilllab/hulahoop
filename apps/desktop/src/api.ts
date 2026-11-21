/** The six columns of the board, plus the states a task can leave it through. */
export type IssueStatus =
  | "backlog"
  | "todo"
  | "in_progress"
  | "in_review"
  | "blocked"
  | "done"
  | "cancelled";

/** The six columns of the board, in the order the workflow document sets. */
export const BOARD_STATUSES: IssueStatus[] = [
  "backlog",
  "todo",
  "in_progress",
  "blocked",
  "in_review",
  "done",
];

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
  id: string;
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
  /** Set when the task is blocked, naming what it waits for. */
  blockedBy: string | null;
  projectName: string | null;
  error: string | null;
  tokensIn: number;
  tokensOut: number;
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
  /** The agent's name, or `you` for a person. */
  author: string;
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
  /** `BAL-15`: the project's prefix and the number inside it. */
  key: string;
  number: number;
  projectId: string | null;
  projectPrefix: string | null;
  milestoneId: string | null;
  title: string;
  body: string;
  status: IssueStatus;
  priority: string;
  attempt: number;
  runCount: number;
  testsFrozenAt: string | null;
  mergeRevision: string | null;
  prUrl: string | null;
  prNumber: number | null;
  pinnedCommentId: string | null;
  /** Set when the task is blocked, naming what it waits for. */
  blockedBy: string | null;
  projectName: string | null;
  criteria: Criterion[];
  runs: Run[];
  reviews: Review[];
  comments: Comment[];
  approval: Approval | null;
  currentRun: Run | null;
}

export interface Repo {
  id: string;
  name: string;
  /** Null when the repository was attached by its remote and lives in a mirror. */
  path: string | null;
  remoteUrl: string | null;
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
  agentsMd: string;
  skills: Skill[];
}

export interface ProjectAgent {
  projectId: string;
  agentId: string;
  role: Role;
}

export interface Project {
  id: string;
  name: string;
  /** `BAL` in `BAL-15`. Null until somebody decides one. */
  prefix: string | null;
  repos: Repo[];
}

export interface Workspace {
  id: string;
  name: string;
  slug: string;
  /** What an issue with no project is called by. */
  defaultPrefix: string;
}

export interface WorkspaceMember {
  userId: string;
  name: string;
  email: string | null;
  role: string;
}

export type WorkflowSection = {
  status: IssueStatus;
  title: string;
  owner: string;
  agent: string | null;
  next: IssueStatus | null;
  body: string;
};

export type WorkflowDoc = {
  source: string;
  updatedAt: string | null;
  sections: WorkflowSection[];
  errors: string[];
};

export interface Skill {
  id: string;
  name: string;
  description: string;
  source: string;
  updatedAt: string;
}

export interface Milestone {
  id: string;
  projectId: string | null;
  title: string;
  body: string;
  status: string;
  targetDate: string | null;
  createdAt: string;
}

export interface Artifact {
  id: string;
  projectId: string | null;
  milestoneId: string | null;
  title: string;
  kind: string;
  filename: string;
  mime: string;
  size: number;
  sha256: string;
  createdAt: string;
}

let connection: Connection | null = null;

const REMOTE_KEY = "hulahoop.remote";

interface SavedRemote {
  url: string;
  token: string;
}

/** A server saved from the connect screen; work runs wherever it listens. */
/** Where the chosen workspace is remembered between reloads. */
export const WORKSPACE_KEY = "hulahoop.workspace";

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
  id key number projectId projectPrefix milestoneId
  title body status priority attempt runCount testsFrozenAt mergeRevision pinnedCommentId
  blockedBy projectName prUrl prNumber
  criteria { id position text testName }
  runs { id role status attempt adapter flags headBookmark headRevision diffSha commitName commitEmail prUrl prNumber error tokensIn tokensOut startedAt finishedAt }
  reviews { id reviewedRunId reviewerRunId verdict body createdAt }
  comments { id author authorType body pinned createdAt }
  currentRun { id role status attempt adapter flags headBookmark headRevision diffSha commitName commitEmail prUrl prNumber error tokensIn tokensOut startedAt finishedAt }
`;

export async function loadBoard(workspaceId: string): Promise<Issue[]> {
  const data = await gql<{ issues: Issue[] }>(
    `query ($workspaceId: UUID!) { issues(workspaceId: $workspaceId) { ${ISSUE_FIELDS} } }`,
    { workspaceId },
  );
  return data.issues;
}

export const SKILL_FIELDS = `id name description source updatedAt`;

export async function loadSkills(workspaceId: string): Promise<Skill[]> {
  const data = await gql<{ skills: Skill[] }>(
    `query ($w: UUID!) { skills(workspaceId: $w) { ${SKILL_FIELDS} } }`,
    { w: workspaceId },
  );
  return data.skills;
}

export async function createSkill(
  workspaceId: string,
  name: string,
  description: string,
  source: string,
): Promise<Skill> {
  const data = await gql<{ createSkill: Skill }>(
    `mutation ($w: UUID!, $n: String!, $d: String, $s: String!) {
      createSkill(workspaceId: $w, name: $n, description: $d, source: $s) { ${SKILL_FIELDS} }
    }`,
    { w: workspaceId, n: name, d: description, s: source },
  );
  return data.createSkill;
}

export async function updateSkill(
  id: string,
  changes: { name?: string; description?: string; source?: string },
): Promise<Skill> {
  const data = await gql<{ updateSkill: Skill }>(
    `mutation ($id: UUID!, $n: String, $d: String, $s: String) {
      updateSkill(id: $id, name: $n, description: $d, source: $s) { ${SKILL_FIELDS} }
    }`,
    { id, n: changes.name ?? null, d: changes.description ?? null, s: changes.source ?? null },
  );
  return data.updateSkill;
}

export async function deleteSkill(id: string): Promise<boolean> {
  const data = await gql<{ deleteSkill: boolean }>(
    `mutation ($id: UUID!) { deleteSkill(id: $id) }`,
    { id },
  );
  return data.deleteSkill;
}

export async function setAgentSkills(agentId: string, skillIds: string[]): Promise<Skill[]> {
  const data = await gql<{ setAgentSkills: Skill[] }>(
    `mutation ($a: UUID!, $s: [UUID!]!) { setAgentSkills(agentId: $a, skillIds: $s) { ${SKILL_FIELDS} } }`,
    { a: agentId, s: skillIds },
  );
  return data.setAgentSkills;
}

export async function setAgentInstructions(agentId: string, agentsMd: string): Promise<boolean> {
  const data = await gql<{ setAgentInstructions: boolean }>(
    `mutation ($a: UUID!, $m: String!) { setAgentInstructions(agentId: $a, agentsMd: $m) }`,
    { a: agentId, m: agentsMd },
  );
  return data.setAgentInstructions;
}

export const MILESTONE_FIELDS = `id projectId title body status targetDate createdAt`;

/** A run seen from the workspace, for Activity and Cost control. */
export interface RecentRun {
  id: string;
  issueId: string;
  agentId: string;
  role: Role;
  status: string;
  adapter: string;
  attempt: number;
  tokensIn: number;
  tokensOut: number;
  error: string | null;
  startedAt: string | null;
  finishedAt: string | null;
}

export async function loadRecentRuns(workspaceId: string, limit = 120): Promise<RecentRun[]> {
  const data = await gql<{ recentRuns: RecentRun[] }>(
    `query ($w: UUID!, $n: Int) { recentRuns(workspaceId: $w, limit: $n) {
      id issueId agentId role status adapter attempt tokensIn tokensOut error startedAt finishedAt
    } }`,
    { w: workspaceId, n: limit },
  );
  return data.recentRuns;
}

export async function loadWorkflow(workspaceId: string): Promise<WorkflowDoc> {
  const data = await gql<{ workflow: WorkflowDoc }>(
    `query ($w: UUID!) { workflow(workspaceId: $w) {
      source updatedAt errors
      sections { status title owner agent next body }
    } }`,
    { w: workspaceId },
  );
  return data.workflow;
}

export async function saveWorkflow(workspaceId: string, source: string): Promise<WorkflowDoc> {
  const data = await gql<{ setWorkflow: WorkflowDoc }>(
    `mutation ($w: UUID!, $s: String!) { setWorkflow(workspaceId: $w, source: $s) {
      source updatedAt errors
      sections { status title owner agent next body }
    } }`,
    { w: workspaceId, s: source },
  );
  return data.setWorkflow;
}

export async function loadMilestones(workspaceId: string): Promise<Milestone[]> {
  const data = await gql<{ milestones: Milestone[] }>(
    `query ($workspaceId: UUID!) { milestones(workspaceId: $workspaceId) { ${MILESTONE_FIELDS} } }`,
    { workspaceId },
  );
  return data.milestones;
}

export const ARTIFACT_FIELDS = `id projectId milestoneId title kind filename mime size sha256 createdAt`;

export async function loadArtifacts(workspaceId: string): Promise<Artifact[]> {
  const data = await gql<{ artifacts: Artifact[] }>(
    `query ($workspaceId: UUID!) { artifacts(workspaceId: $workspaceId) { ${ARTIFACT_FIELDS} } }`,
    { workspaceId },
  );
  return data.artifacts;
}

/** Upload a file: base64 in the mutation body, bytes on the server's disk. */
export async function uploadArtifact(input: {
  workspaceId: string;
  projectId: string | null;
  milestoneId: string | null;
  title: string;
  file: File;
}): Promise<Artifact> {
  const buffer = new Uint8Array(await input.file.arrayBuffer());
  let binary = "";
  const chunk = 0x8000;
  for (let index = 0; index < buffer.length; index += chunk) {
    binary += String.fromCharCode(...buffer.subarray(index, index + chunk));
  }
  const data = await gql<{ createArtifact: Artifact }>(
    `mutation ($workspaceId: UUID!, $projectId: UUID, $milestoneId: UUID, $title: String, $filename: String!, $mime: String, $contentBase64: String!) {
      createArtifact(workspaceId: $workspaceId, projectId: $projectId, milestoneId: $milestoneId, title: $title, filename: $filename, mime: $mime, contentBase64: $contentBase64) { ${ARTIFACT_FIELDS} }
    }`,
    {
      workspaceId: input.workspaceId,
      projectId: input.projectId,
      milestoneId: input.milestoneId,
      title: input.title,
      filename: input.file.name,
      mime: input.file.type || null,
      contentBase64: btoa(binary),
    },
  );
  return data.createArtifact;
}

/** Fetch the bytes with the bearer token, then hand them to the browser to save. */
export async function downloadArtifact(artifact: Artifact): Promise<void> {
  const current = await connect();
  const response = await fetch(`${current.url}/artifacts/${artifact.id}`, {
    headers: { authorization: `Bearer ${current.token}` },
  });
  if (!response.ok) throw new Error(`the server answered ${response.status} for ${artifact.filename}`);
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = artifact.filename;
  link.click();
  URL.revokeObjectURL(url);
}

export async function loadIssue(id: string): Promise<Issue> {
  const data = await gql<{ issue: Issue }>(`query ($id: UUID!) { issue(id: $id) { ${ISSUE_FIELDS} } }`, { id });
  return data.issue;
}

export interface WorkspaceSnapshot {
  workspaces: Workspace[];
  workspace: Workspace;
  projects: Project[];
  agents: Agent[];
}

/**
 * Workspaces are the top of the tree, and a server can hold more than one. The
 * chosen id is remembered, so a reload lands where the last one left off.
 */
export async function loadWorkspace(workspaceId?: string | null): Promise<WorkspaceSnapshot> {
  const data = await gql<{ workspaces: Workspace[] }>(
    `{ workspaces { id name slug defaultPrefix } }`,
  );
  const workspace =
    data.workspaces.find((entry) => entry.id === workspaceId) ?? data.workspaces[0];
  if (!workspace) throw new Error("the server has no workspace yet");
  const projects = await gql<{ projects: Project[] }>(
    `query ($id: UUID!) { projects(workspaceId: $id) { id name prefix repos { id name path vcs baseBookmark contractVersion fetchedAt } } }`,
    { id: workspace.id },
  );
  const agents = await gql<{ agents: Agent[] }>(
    `query ($id: UUID!) { agents(workspaceId: $id) {
      id name adapter model agentsMd
      skills { ${SKILL_FIELDS} }
    } }`,
    { id: workspace.id },
  );
  return {
    workspaces: data.workspaces,
    workspace,
    projects: projects.projects,
    agents: agents.agents,
  };
}

export async function createWorkspace(name: string, prefix?: string): Promise<Workspace> {
  const data = await gql<{ createWorkspace: Workspace }>(
    `mutation ($name: String!, $prefix: String) { createWorkspace(name: $name, prefix: $prefix) { id name slug defaultPrefix } }`,
    { name, prefix: prefix?.trim() ? prefix.trim() : null },
  );
  return data.createWorkspace;
}

export async function updateWorkspace(
  id: string,
  name?: string,
  defaultPrefix?: string,
): Promise<Workspace> {
  const data = await gql<{ updateWorkspace: Workspace }>(
    `mutation ($id: UUID!, $name: String, $defaultPrefix: String) {
       updateWorkspace(id: $id, name: $name, defaultPrefix: $defaultPrefix) { id name slug defaultPrefix }
     }`,
    { id, name: name ?? null, defaultPrefix: defaultPrefix ?? null },
  );
  return data.updateWorkspace;
}

export async function renameProject(id: string, name: string): Promise<Project> {
  const data = await gql<{ renameProject: Project }>(
    `mutation ($id: UUID!, $name: String!) { renameProject(id: $id, name: $name) { id name prefix } }`,
    { id, name },
  );
  return data.renameProject;
}

export interface GithubRepo {
  fullName: string;
  name: string;
  owner: string;
  private: boolean;
  archived: boolean;
  defaultBranch: string;
  cloneUrl: string;
}

/// Every repository the installed GitHub app can reach. Reads the stored app key,
/// mints the app JWT on the server, and never exposes a token to the browser.
export async function loadGithubRepos(): Promise<GithubRepo[]> {
  const data = await gql<{ githubRepos: GithubRepo[] }>(
    `query { githubRepos { fullName name owner private archived defaultBranch cloneUrl } }`,
  );
  return data.githubRepos;
}

/// Attach a repository the app can reach. It has no checkout on this machine, so
/// the runner clones into a mirror it owns on the first run.
export async function attachGithubRepo(projectId: string, repo: GithubRepo): Promise<string> {
  const data = await gql<{ addRepo: { id: string } }>(
    `mutation ($projectId: UUID!, $name: String!, $remoteUrl: String!) {
       addRepo(projectId: $projectId, name: $name, remoteUrl: $remoteUrl) { id }
     }`,
    { projectId, name: repo.name, remoteUrl: repo.cloneUrl },
  );
  return data.addRepo.id;
}

export async function loadWorkspaceMembers(workspaceId: string): Promise<WorkspaceMember[]> {
  const data = await gql<{ workspaceMembers: WorkspaceMember[] }>(
    `query ($id: UUID!) { workspaceMembers(workspaceId: $id) { userId name email role } }`,
    { id: workspaceId },
  );
  return data.workspaceMembers;
}

export async function loadWorkspaceIssues(workspaceId: string): Promise<Issue[]> {
  return loadBoard(workspaceId);
}
