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

export interface Connection {
  /** The server's GraphQL endpoint root, like `http://127.0.0.1:51234`. */
  url: string;
  token: string;
  dataDir: string;
  /** What to show in the rail: a host name, or `local`. */
  label: string;
}

export interface Run {
  id: string;
  /** Who ran it. Every run records its agent, so a roster can read its own history. */
  agentId: string;
  status: string;
  /** The turn whose work this one started from; running an agent again reuses it. */
  parentRunId?: string | null;
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
  /** The agent's own closing note, markdown. Only the detail query asks for it. */
  summary?: string | null;
  model?: string | null;
  actualModel?: string | null;
  modelTurns?: number | null;
  toolCalls?: number | null;
  apiEquivalentUsd?: number | null;
  cacheReadTokens?: number | null;
  failureCategory?: string | null;
  createdAt?: string;
  /** What the run signed in with: a connector name such as `Claude subscription`,
   *  or `host login`. Null for runs from before it was recorded. */
  credentialSource?: string | null;
  /** Why a queued run has not started yet: `Waiting for memory: needs ~2.4 GB,
   *  1.1 GB free`. Null once it runs. */
  waitReason?: string | null;
  /** What the run used of the host, as the runner measured it. */
  peakMemoryBytes?: number | null;
  cpuCores?: number | null;
  diskGrowthBytes?: number | null;
}

/** One normalized line of a run's transcript. */
export interface TranscriptEntry {
  seq: number;
  /** `prompt`, `thinking`, `text`, `tool_call`, `tool_result`, `error` or `done`. */
  kind: string;
  text: string;
  tool: string | null;
  input: string | null;
  output: string | null;
  toolId: string | null;
  isError: boolean;
}

export interface TranscriptPage {
  entries: TranscriptEntry[];
  lastSeq: number;
  live: boolean;
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
  /** The run that wrote it, for an agent's summary, question or verdict. */
  runId?: string | null;
  mentions?: Mention[];
}

export interface Mention {
  type: "agent" | "lumi";
  id: string;
  name: string;
}

export interface Assignee {
  type: "human" | "agent";
  id: string;
  name: string;
}

export interface IssueStats {
  runs: number;
  failedRuns: number;
  turns: number;
  toolCalls: number;
  tokens: number;
  apiEquivalentUsd: number;
  activeMinutes: number;
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
  /** `YYYY-MM-DD`, or null. */
  dueDate: string | null;
  assignee?: Assignee | null;
  fileCount?: number;
  stats?: IssueStats;
  attempt: number;
  runCount: number;
  mergeRevision: string | null;
  prUrl: string | null;
  prNumber: number | null;
  pinnedCommentId: string | null;
  /** Set when the task is blocked, naming what it waits for. */
  blockedBy: string | null;
  projectName: string | null;
  /** Who still owes a turn in the current status, in the workflow's order. */
  turnRemaining?: string[];
  /** Where the workflow document says this status may go. */
  moves?: IssueStatus[];
  turnsUsed?: number;
  /** Runs used since a person last allowed more; at `runLimit` no agent starts. */
  runsSinceCap?: number;
  runLimit?: number;
  /** The task's attached files; only the detail query carries them. */
  files?: Artifact[];
  runs: Run[];
  reviews: Review[];
  comments: Comment[];
  currentRun: Run | null;
}

/** What the board and the list draw: scalars, a file count, the declared moves and
 *  whether a run is live. The body and the turn queue only matter to the open task. */
export type IssueSummary = Omit<
  Issue,
  "body" | "turnRemaining" | "turnsUsed" | "runsSinceCap" | "runLimit" | "runs" | "reviews" | "comments" | "currentRun" | "files" | "stats"
> & {
  currentRun: Pick<Run, "id" | "agentId" | "status" | "waitReason"> | null;
};

export interface Repo {
  id: string;
  name: string;
  /** The url Hulahoop clones the repository from into its own mirror. */
  remoteUrl: string | null;
  vcs: string;
  baseBookmark: string;
  contractVersion: string;
  fetchedAt: string | null;
}

export interface Agent {
  /** Which connector this agent signs in with, where its harness offers two. */
  connector: string | null;
  id: string;
  name: string;
  adapter: string;
  model: string;
  agentsMd: string;
  skills: Skill[];
}

/** A model a harness offers, with the price a cost estimate needs. */
export interface ModelOption {
  id: string;
  label: string;
  provider: string;
  costIn: number | null;
  costOut: number | null;
  contextWindow: number | null;
}

export interface Project {
  id: string;
  name: string;
  /** `BAL` in `BAL-15`. Null until somebody decides one. */
  prefix: string | null;
  /** One of the eight palette colours, as `#rrggbb`. */
  color: string;
  repos: Repo[];
}

export interface Workspace {
  id: string;
  name: string;
  slug: string;
  /** What an issue with no project is called by. */
  defaultPrefix: string;
  /** The harness and model Lumi answers with. Null until somebody picks one. */
  lumiHarness: string | null;
  lumiModel: string | null;
}

/** One checkbox of a Lumi proposal: what will happen, in words and in action. */
export interface LumiProposalItem {
  group: string;
  text: string;
  action: string;
  args: Record<string, unknown>;
}

export interface LumiProposal {
  title: string;
  items: LumiProposalItem[];
}

/** One turn of the Lumi conversation. A `lumi` row may carry a proposal. */
export interface LumiMessage {
  id: string;
  role: "user" | "lumi";
  body: string;
  context: string | null;
  /** JSON text of a LumiProposal, or null. */
  proposal: string | null;
  proposalState: "open" | "applied" | "dismissed" | null;
  createdAt: string;
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
  /// The `Users:` line as written: `human`, `@Implementer`, `any`.
  users: string[];
  agents: string[];
  anyAgent: boolean;
  next: IssueStatus | null;
  nexts: IssueStatus[];
  /// Each move with its own reason: what the arrows and the table print.
  transitions: { to: IssueStatus; title: string; when: string | null }[];
  /// The sentence the arrow shows and the transitions table prints.
  when: string | null;
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
  /** 1 for the skills a workspace is born with. They are read, never edited. */
  builtin: number;
  updatedAt: string;
}

/** A note the agents (or a person) keep across turns and tasks. */
export interface Memory {
  id: string;
  scope: "workspace" | "project" | "person" | "agent";
  /** The project id for `project`, a name for `agent` and `person`. */
  scopeRef: string;
  kind: "decision" | "convention" | "preference" | "fact" | "gotcha";
  title: string;
  body: string;
  sourceIssueId: string | null;
  sourceIssueKey: string | null;
  author: string;
  updatedAt: string;
  useCount: number;
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
  /** The task the file is attached to, when it is. */
  issueId: string | null;
  title: string;
  kind: string;
  filename: string;
  mime: string;
  size: number;
  sha256: string;
  createdAt: string;
  /** Plan, spec, report, notes or a plain file. */
  type: "plan" | "spec" | "report" | "notes" | "file";
  /** Markdown body; empty for a plain file. */
  content: string;
  files: ArtifactFile[];
  /** Person or agent who wrote it. */
  author: string;
  authorIsAgent: boolean;
  updatedAt: string;
}

export interface ArtifactFile {
  id: string;
  filename: string;
  mime: string;
  size: number;
}

let connection: Connection | null = null;

const REMOTE_KEY = "hulahoop.remote";

interface SavedRemote {
  url: string;
  token: string;
}

/** A server saved from the connect screen; work runs wherever it listens. */
/** Where the chosen workspace is remembered between reloads. */
/// Calls a Rust command. Only the desktop shell has them, so every caller must
/// tolerate a failure: the same screen has to work in a plain browser tab.
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const bridge = (window as unknown as { __TAURI_INTERNALS__?: { invoke: (c: string, a?: unknown) => Promise<T> } })
    .__TAURI_INTERNALS__;
  if (!bridge) throw new Error("this build has no desktop shell");
  return (await bridge.invoke(command, args)) as T;
}

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

const RUN_FIELDS = `id agentId status attempt adapter flags headBookmark headRevision diffSha commitName commitEmail prUrl prNumber error tokensIn tokensOut startedAt finishedAt credentialSource waitReason peakMemoryBytes cpuCores diskGrowthBytes`;

/** The light shape every list of tasks asks for, every few seconds: no body,
 *  no turn queue, no nested runs or comments. */
export const ISSUE_SUMMARY_FIELDS = `
  id key number projectId projectPrefix milestoneId
  title status priority dueDate attempt runCount mergeRevision pinnedCommentId
  blockedBy projectName prUrl prNumber fileCount
  assignee { type id name }
  moves
  currentRun { id agentId status waitReason }
`;

/** Everything the task panel shows, fetched for the one task that is open. */
export const ISSUE_DETAIL_FIELDS = `
  id key number projectId projectPrefix milestoneId
  title body status priority dueDate attempt runCount mergeRevision pinnedCommentId
  blockedBy projectName prUrl prNumber fileCount
  assignee { type id name }
  stats { runs failedRuns turns toolCalls tokens apiEquivalentUsd activeMinutes }
  files { id projectId milestoneId issueId title kind filename mime size sha256 createdAt }
  runs { ${RUN_FIELDS} summary model actualModel modelTurns toolCalls apiEquivalentUsd cacheReadTokens failureCategory parentRunId createdAt }
  reviews { id reviewedRunId reviewerRunId verdict body createdAt }
  comments(newestFirst: true) { id author authorType body pinned createdAt runId mentions { type id name } }
  turnRemaining turnsUsed runsSinceCap runLimit moves
  currentRun { ${RUN_FIELDS} }
`;

/** How many runs share the host and the room the runner last reported. */
export interface RunnerCapacity {
  running: number;
  waiting: number;
  freeMemoryBytes: number;
  freeDiskBytes: number;
  freeCpuCores: number;
  maxRuns: number | null;
  reportedAt: string;
}

/** Null when no runner has reported in the last thirty seconds. */
export async function loadRunnerCapacity(): Promise<RunnerCapacity | null> {
  const data = await gql<{ runnerCapacity: RunnerCapacity | null }>(
    `query { runnerCapacity { running waiting freeMemoryBytes freeDiskBytes freeCpuCores maxRuns reportedAt } }`,
  );
  return data.runnerCapacity;
}

/** Bytes the way the board writes them: decimal gigabytes, one decimal. */
export function fmtGb(bytes: number): string {
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

/** The workspace's tasks. Done and cancelled ones stay on the server unless asked for. */
export async function loadBoard(workspaceId: string, includeDone = false): Promise<IssueSummary[]> {
  const data = await gql<{ issues: IssueSummary[] }>(
    `query ($workspaceId: UUID!, $includeDone: Boolean!) { issues(workspaceId: $workspaceId, includeDone: $includeDone) { ${ISSUE_SUMMARY_FIELDS} } }`,
    { workspaceId, includeDone },
  );
  return data.issues;
}

/** How many done tasks the Active view leaves out. `projectFilter` is "all", "none" (no project) or a project id. */
export async function loadDoneIssueCount(workspaceId: string, projectFilter: string): Promise<number> {
  const data = await gql<{ doneIssueCount: number }>(
    `query ($workspaceId: UUID!, $projectIds: [UUID!], $noProject: Boolean!) { doneIssueCount(workspaceId: $workspaceId, projectIds: $projectIds, noProject: $noProject) }`,
    {
      workspaceId,
      projectIds: projectFilter === "all" || projectFilter === "none" ? [] : [projectFilter],
      noProject: projectFilter === "none",
    },
  );
  return data.doneIssueCount;
}

/** The task asked for is gone (deleted), as opposed to a failed request. */
export class IssueGoneError extends Error {}

export async function loadIssueDetail(id: string): Promise<Issue> {
  const data = await gql<{ issue: Issue | null }>(`query ($id: UUID!) { issue(id: $id) { ${ISSUE_DETAIL_FIELDS} } }`, { id });
  if (!data.issue) throw new IssueGoneError("This task no longer exists.");
  return data.issue;
}

/** A page of a run's transcript after `afterSeq`; poll with `lastSeq` while `live`. */
export async function loadTranscript(runId: string, afterSeq: number): Promise<TranscriptPage> {
  const data = await gql<{ run: { transcript: TranscriptPage } | null }>(
    `query ($id: UUID!, $afterSeq: Int!) { run(id: $id) { transcript(afterSeq: $afterSeq) { entries { seq kind text tool input output toolId isError } lastSeq live } } }`,
    { id: runId, afterSeq },
  );
  return data.run?.transcript ?? { entries: [], lastSeq: afterSeq, live: false };
}

export interface CreateIssueInput {
  workspaceId: string;
  projectId: string | null;
  repoId: string | null;
  title: string;
  body: string;
  priority: string;
  dueDate: string | null;
  assigneeType: "human" | "agent" | null;
  assigneeId: string | null;
  /** A milestone of the same workspace, or none. */
  milestoneId: string | null;
}

export async function createIssue(input: CreateIssueInput): Promise<IssueSummary> {
  const data = await gql<{ createIssue: IssueSummary }>(
    `mutation ($workspaceId: UUID!, $projectId: UUID, $repoId: UUID, $title: String!, $body: String!, $priority: String, $dueDate: String, $assigneeType: String, $assigneeId: String, $milestoneId: UUID) {
      createIssue(workspaceId: $workspaceId, projectId: $projectId, repoId: $repoId, title: $title, body: $body, priority: $priority, dueDate: $dueDate, assigneeType: $assigneeType, assigneeId: $assigneeId, milestoneId: $milestoneId) { ${ISSUE_SUMMARY_FIELDS} }
    }`,
    { ...input },
  );
  return data.createIssue;
}

/**
 * Only the keys present change; `null` clears due date and assignee.
 *
 * The server reads an argument that is written into the document but has no
 * variable as `null`, and for `dueDate` and `assigneeType` that means clear.
 * So each group gets its own document that names only its own arguments.
 */
export async function updateIssue(
  id: string,
  patch: {
    title?: string;
    body?: string;
    priority?: string;
    dueDate?: string | null;
    assigneeType?: string | null;
    assigneeId?: string | null;
    /** A project brings its first repository; `null` leaves the task with no project and no checkout. */
    projectId?: string | null;
  },
): Promise<void> {
  const { title, body, priority } = patch;
  if (title !== undefined || body !== undefined || priority !== undefined) {
    await gql(
      `mutation ($id: UUID!, $title: String, $body: String, $priority: String) {
        updateIssue(id: $id, title: $title, body: $body, priority: $priority) { id }
      }`,
      { id, title, body, priority },
    );
  }
  if ("dueDate" in patch) {
    await gql(`mutation ($id: UUID!, $dueDate: String) { updateIssue(id: $id, dueDate: $dueDate) { id } }`, {
      id,
      dueDate: patch.dueDate ?? null,
    });
  }
  if ("assigneeType" in patch) {
    await gql(
      `mutation ($id: UUID!, $assigneeType: String, $assigneeId: String) {
        updateIssue(id: $id, assigneeType: $assigneeType, assigneeId: $assigneeId) { id }
      }`,
      { id, assigneeType: patch.assigneeType ?? null, assigneeId: patch.assigneeId ?? null },
    );
  }
  if ("projectId" in patch) {
    await gql(`mutation ($id: UUID!, $projectId: UUID) { updateIssue(id: $id, projectId: $projectId) { id } }`, {
      id,
      projectId: patch.projectId ?? null,
    });
  }
}

export async function moveIssue(issueId: string, status: string): Promise<void> {
  await gql(`mutation ($issueId: UUID!, $status: String!) { moveIssue(issueId: $issueId, status: $status) { id } }`, {
    issueId,
    status,
  });
}

export async function addComment(issueId: string, body: string): Promise<void> {
  await gql(`mutation ($issueId: UUID!, $body: String!) { addComment(issueId: $issueId, body: $body) { id } }`, {
    issueId,
    body,
  });
}

export async function deleteArtifact(id: string): Promise<boolean> {
  const data = await gql<{ deleteArtifact: boolean }>(`mutation ($id: UUID!) { deleteArtifact(id: $id) }`, { id });
  return data.deleteArtifact;
}

export const SKILL_FIELDS = `id name description source builtin updatedAt`;

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

export const MEMORY_FIELDS = `id scope scopeRef kind title body sourceIssueId sourceIssueKey author updatedAt useCount`;

/** The workspace's live memories, narrowed by scope and by words. */
export async function loadMemories(
  workspaceId: string,
  filter: { scope?: string; query?: string } = {},
): Promise<Memory[]> {
  const data = await gql<{ memories: Memory[] }>(
    `query ($w: UUID!, $s: String, $q: String) {
      memories(workspaceId: $w, scope: $s, query: $q) { ${MEMORY_FIELDS} }
    }`,
    { w: workspaceId, s: filter.scope ?? null, q: filter.query?.trim() || null },
  );
  return data.memories;
}

export async function updateMemory(
  id: string,
  fields: { kind: string; title: string; body: string },
): Promise<Memory> {
  const data = await gql<{ updateMemory: Memory }>(
    `mutation ($id: UUID!, $k: String, $t: String, $b: String) {
      updateMemory(id: $id, kind: $k, title: $t, body: $b) { ${MEMORY_FIELDS} }
    }`,
    { id, k: fields.kind, t: fields.title, b: fields.body },
  );
  return data.updateMemory;
}

export async function forgetMemory(id: string): Promise<boolean> {
  const data = await gql<{ forgetMemory: boolean }>(
    `mutation ($id: UUID!) { forgetMemory(id: $id) }`,
    { id },
  );
  return data.forgetMemory;
}

/** Which agents carry which skills. The library shows this before it lets you
 *  write another skill for something already loaded. */
export async function loadSkillUsage(workspaceId: string): Promise<Record<string, string[]>> {
  const data = await gql<{ skillUsage: { skillId: string; agentNames: string[] }[] }>(
    `query ($w: UUID!) { skillUsage(workspaceId: $w) { skillId agentNames } }`,
    { w: workspaceId },
  );
  return Object.fromEntries(data.skillUsage.map((row) => [row.skillId, row.agentNames]));
}

/** The workspace's agents with the skills each one carries, for the Used-by
 *  toggles on the Skills page. */
export async function loadAgentSkills(
  workspaceId: string,
): Promise<{ id: string; name: string; skills: Skill[] }[]> {
  const data = await gql<{ agents: { id: string; name: string; skills: Skill[] }[] }>(
    `query ($w: UUID!) { agents(workspaceId: $w) { id name skills { id } } }`,
    { w: workspaceId },
  );
  return data.agents;
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
  status: string;
  adapter: string;
  /** The model the run was given, frozen at spawn. */
  model: string;
  attempt: number;
  tokensIn: number;
  tokensOut: number;
  error: string | null;
  startedAt: string | null;
  finishedAt: string | null;
  headBookmark?: string | null;
}

export async function loadRecentRuns(workspaceId: string, limit = 120): Promise<RecentRun[]> {
  const data = await gql<{ recentRuns: RecentRun[] }>(
    `query ($w: UUID!, $n: Int) { recentRuns(workspaceId: $w, limit: $n) {
      id issueId agentId status adapter model attempt tokensIn tokensOut error startedAt finishedAt headBookmark
    } }`,
    { w: workspaceId, n: limit },
  );
  return data.recentRuns;
}

export interface CostTotals {
  runs: number;
  succeeded: number;
  failed: number;
  cancelled: number;
  tokensIn: number;
  tokensOut: number;
  cacheRead: number;
  cacheWrite: number;
  knownCostRuns: number;
  equivalentUsd: number;
  knownCashRuns: number;
  cashUsd: number;
  activeRuns: number;
  activeMinutes: number;
  knownTurnRuns: number;
  turns: number;
  knownToolRuns: number;
  tools: number;
}

export interface CostCohort {
  tasks: number;
  runs: number;
  failedRuns: number;
  elapsedMinutes: number;
  activeRuns: number;
  activeMinutes: number;
  knownCostRuns: number;
  equivalentUsd: number;
  knownCashRuns: number;
  cashUsd: number;
  knownTurnRuns: number;
  turns: number;
  knownToolRuns: number;
  tools: number;
}

export interface CostReport {
  period: "today" | "7d" | "30d" | "mtd";
  start: string;
  end: string;
  priorStart: string;
  current: CostTotals;
  previous: CostTotals;
  tasks: CostCohort;
  priorTasks: CostCohort;
  month: CostTotals;
  planUsage: (CostTotals & { label: string })[];
  monthElapsedDays: number;
  monthTotalDays: number;
  unsuccessfulTasks: number;
  daily: (CostTotals & { label: string })[];
  dailyModels: (CostTotals & { day: string; model: string })[];
  agents: (CostTotals & { label: string })[];
  models: (CostTotals & { label: string })[];
  failureReasons: (CostTotals & { label: string })[];
  failures: { id: string; issueId: string; issueKey: string; issueTitle: string;
    agent: string; model: string; reason: string; finishedAt: string; apiEquivalentUsd: number | null }[];
  settings: { apiBudgetUsd: number | null; successTargetPercent: number | null };
  plans: { connectorId: string; label: string; monthlyFeeUsd: number; allowanceTokens: number | null }[];
}

export async function loadCostReport(workspaceId: string, period: CostReport["period"], agentId: string | null, model: string | null): Promise<CostReport> {
  const data = await gql<{ costReport: CostReport }>(
    `query ($w: UUID!, $p: String!, $a: UUID, $m: String) {
      costReport(workspaceId: $w, period: $p, agentId: $a, model: $m)
    }`,
    { w: workspaceId, p: period, a: agentId, m: model },
  );
  return data.costReport;
}

export async function saveCostSettings(workspaceId: string, budget: number | null, target: number | null): Promise<void> {
  await gql(`mutation ($w: UUID!, $b: Float, $t: Float) {
    saveCostSettings(workspaceId: $w, apiBudgetUsd: $b, successTargetPercent: $t)
  }`, { w: workspaceId, b: budget, t: target });
}

export async function saveCostPlan(workspaceId: string, connector: string, label: string, fee: number, allowance: number | null): Promise<void> {
  await gql(`mutation ($w: UUID!, $c: String!, $l: String!, $f: Float!, $a: Int) {
    saveCostPlan(workspaceId: $w, connectorId: $c, label: $l, monthlyFeeUsd: $f, allowanceTokens: $a)
  }`, { w: workspaceId, c: connector, l: label, f: fee, a: allowance });
}

const WORKFLOW_FIELDS = `source updatedAt errors
      sections { status title owner agent users agents anyAgent next nexts when body
        transitions { to title when } }`;

export async function loadWorkflow(workspaceId: string): Promise<WorkflowDoc> {
  const data = await gql<{ workflow: WorkflowDoc }>(
    `query ($w: UUID!) { workflow(workspaceId: $w) { ${WORKFLOW_FIELDS} } }`,
    { w: workspaceId },
  );
  return data.workflow;
}

export async function saveWorkflow(workspaceId: string, source: string): Promise<WorkflowDoc> {
  const data = await gql<{ setWorkflow: WorkflowDoc }>(
    `mutation ($w: UUID!, $s: String!) { setWorkflow(workspaceId: $w, source: $s) { ${WORKFLOW_FIELDS} } }`,
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

export const ARTIFACT_FIELDS = `id projectId milestoneId issueId title kind type content author authorIsAgent filename mime size sha256 createdAt updatedAt files { id filename mime size }`;

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
  issueId?: string | null;
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
    `mutation ($workspaceId: UUID!, $projectId: UUID, $milestoneId: UUID, $issueId: UUID, $title: String, $filename: String!, $mime: String, $contentBase64: String!) {
      createArtifact(workspaceId: $workspaceId, projectId: $projectId, milestoneId: $milestoneId, issueId: $issueId, title: $title, filename: $filename, mime: $mime, contentBase64: $contentBase64) { ${ARTIFACT_FIELDS} }
    }`,
    {
      workspaceId: input.workspaceId,
      projectId: input.projectId,
      milestoneId: input.milestoneId,
      issueId: input.issueId ?? null,
      title: input.title,
      filename: input.file.name,
      mime: input.file.type || null,
      contentBase64: btoa(binary),
    },
  );
  return data.createArtifact;
}

async function toBase64(file: File): Promise<string> {
  const buffer = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let index = 0; index < buffer.length; index += 0x8000) {
    binary += String.fromCharCode(...buffer.subarray(index, index + 0x8000));
  }
  return btoa(binary);
}

/** Attach files to an artifact; returns the artifact as the server now holds it. */
export async function addArtifactFiles(artifactId: string, files: File[]): Promise<void> {
  for (const file of files) {
    await gql(
      `mutation ($artifactId: UUID!, $filename: String!, $mime: String, $contentBase64: String!) {
        addArtifactFile(artifactId: $artifactId, filename: $filename, mime: $mime, contentBase64: $contentBase64) { id }
      }`,
      { artifactId, filename: file.name, mime: file.type || null, contentBase64: await toBase64(file) },
    );
  }
}

/** The artifact was created but a file upload failed; it exists and can be opened to add the rest. */
export class PartialArtifactError extends Error {
  constructor(
    readonly artifact: Artifact,
    cause: unknown,
  ) {
    super(cause instanceof Error ? cause.message : String(cause));
  }
}

/** A document artifact owned by a task or a milestone, with any files dropped on it. */
export async function createArtifact(input: {
  workspaceId: string;
  title: string;
  type: Artifact["type"];
  content: string;
  issueId: string | null;
  milestoneId: string | null;
  files: File[];
}): Promise<Artifact> {
  const { files, ...fields } = input;
  const data = await gql<{ createArtifactDocument: Artifact }>(
    `mutation ($workspaceId: UUID!, $title: String!, $type: String!, $content: String, $issueId: UUID, $milestoneId: UUID) {
      createArtifactDocument(workspaceId: $workspaceId, title: $title, type: $type, content: $content, issueId: $issueId, milestoneId: $milestoneId) { ${ARTIFACT_FIELDS} }
    }`,
    fields,
  );
  try {
    await addArtifactFiles(data.createArtifactDocument.id, files);
  } catch (cause) {
    throw new PartialArtifactError(data.createArtifactDocument, cause);
  }
  return data.createArtifactDocument;
}

export async function updateArtifact(id: string, fields: { title?: string; type?: Artifact["type"]; content?: string }): Promise<void> {
  await gql(
    `mutation ($id: UUID!, $title: String, $type: String, $content: String) { updateArtifact(id: $id, title: $title, type: $type, content: $content) { id } }`,
    { id, ...fields },
  );
}

export async function moveArtifact(id: string, owner: { issueId: string | null; milestoneId: string | null }): Promise<void> {
  await gql(
    `mutation ($id: UUID!, $issueId: UUID, $milestoneId: UUID) { moveArtifact(id: $id, issueId: $issueId, milestoneId: $milestoneId) { id } }`,
    { id, ...owner },
  );
}

export async function removeArtifactFile(id: string): Promise<void> {
  await gql(`mutation ($id: UUID!) { removeArtifactFile(id: $id) }`, { id });
}

/** The file's bytes, fetched with the bearer token; previews read these too. */
export async function fetchArtifactBlob(artifact: Pick<Artifact, "id" | "filename">): Promise<Blob> {
  const current = await connect();
  const response = await fetch(`${current.url}/artifacts/${artifact.id}`, {
    headers: { authorization: `Bearer ${current.token}` },
  });
  if (!response.ok) throw new Error(`the server answered ${response.status} for ${artifact.filename}`);
  return response.blob();
}

/** Fetch the bytes with the bearer token, then hand them to the browser to save. */
export async function downloadArtifact(artifact: Pick<Artifact, "id" | "filename">): Promise<void> {
  const blob = await fetchArtifactBlob(artifact);
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = artifact.filename;
  link.click();
  URL.revokeObjectURL(url);
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
    `{ workspaces { id name slug defaultPrefix lumiHarness lumiModel } }`,
  );
  const workspace =
    data.workspaces.find((entry) => entry.id === workspaceId) ?? data.workspaces[0];
  if (!workspace) throw new Error("the server has no workspace yet");
  const projects = await gql<{ projects: Project[] }>(
    `query ($id: UUID!) { projects(workspaceId: $id) { id name prefix color repos { id name remoteUrl vcs baseBookmark contractVersion fetchedAt } } }`,
    { id: workspace.id },
  );
  const agents = await gql<{ agents: Agent[] }>(
    `query ($id: UUID!) { agents(workspaceId: $id) {
      id name adapter model agentsMd connector
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

const LUMI_FIELDS = `id role body context proposal proposalState createdAt`;

export async function loadLumiMessages(workspaceId: string): Promise<LumiMessage[]> {
  const data = await gql<{ lumiMessages: LumiMessage[] }>(
    `query ($id: UUID!) { lumiMessages(workspaceId: $id) { ${LUMI_FIELDS} } }`,
    { id: workspaceId },
  );
  return data.lumiMessages;
}

/** The question lands at once; the answer arrives as its own message later. */
export async function askLumi(
  workspaceId: string,
  text: string,
  context?: string,
): Promise<LumiMessage> {
  const data = await gql<{ askLumi: LumiMessage }>(
    `mutation ($id: UUID!, $text: String!, $context: String) {
       askLumi(workspaceId: $id, text: $text, context: $context) { ${LUMI_FIELDS} }
     }`,
    { id: workspaceId, text, context: context ?? null },
  );
  return data.askLumi;
}

export async function applyLumiProposal(
  messageId: string,
  items: number[],
): Promise<LumiMessage> {
  const data = await gql<{ applyLumiProposal: LumiMessage }>(
    `mutation ($id: UUID!, $items: [Int!]!) {
       applyLumiProposal(messageId: $id, items: $items) { ${LUMI_FIELDS} }
     }`,
    { id: messageId, items },
  );
  return data.applyLumiProposal;
}

export async function dismissLumiProposal(messageId: string): Promise<LumiMessage> {
  const data = await gql<{ dismissLumiProposal: LumiMessage }>(
    `mutation ($id: UUID!) { dismissLumiProposal(messageId: $id) { ${LUMI_FIELDS} } }`,
    { id: messageId },
  );
  return data.dismissLumiProposal;
}

export async function setWorkspaceLumi(
  workspaceId: string,
  harness: string,
  model: string,
): Promise<Workspace> {
  const data = await gql<{ setWorkspaceLumi: Workspace }>(
    `mutation ($id: UUID!, $harness: String!, $model: String!) {
       setWorkspaceLumi(workspaceId: $id, harness: $harness, model: $model) {
         id name slug defaultPrefix lumiHarness lumiModel
       }
     }`,
    { id: workspaceId, harness, model },
  );
  return data.setWorkspaceLumi;
}

export interface NewRepo {
  name: string;
  remoteUrl: string;
  baseBookmark: string;
}

/** The project, and the repositories it links, save together or not at all. */
export async function createProject(
  workspaceId: string,
  input: { name: string; prefix: string; color: string; repos: NewRepo[] },
): Promise<string> {
  const data = await gql<{ createProject: { id: string } }>(
    `mutation ($workspaceId: UUID!, $name: String!, $prefix: String!, $color: String!, $repos: [NewRepoInput!]) {
       createProject(workspaceId: $workspaceId, name: $name, prefix: $prefix, color: $color, repos: $repos) { id }
     }`,
    { workspaceId, ...input },
  );
  return data.createProject.id;
}

export async function updateProject(id: string, name: string, color: string): Promise<void> {
  await gql(
    `mutation ($id: UUID!, $name: String!, $color: String!) { updateProject(id: $id, name: $name, color: $color) { id } }`,
    { id, name, color },
  );
}

export async function setProjectPrefix(projectId: string, prefix: string): Promise<void> {
  await gql(
    `mutation ($projectId: UUID!, $prefix: String!) { setProjectPrefix(projectId: $projectId, prefix: $prefix) { id } }`,
    { projectId, prefix },
  );
}

/// Take a repository off its project. The server refuses while open tasks use it or it has run history.
export async function unlinkRepo(id: string): Promise<void> {
  await gql(`mutation ($id: UUID!) { unlinkRepo(id: $id) { id } }`, { id });
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

/// Link a repository by its url. It has no checkout on this machine, so the
/// runner clones into a mirror it owns on the first run.
export async function addRepo(projectId: string, repo: NewRepo): Promise<void> {
  const data = await gql<{ addRepo: { id: string } }>(
    `mutation ($projectId: UUID!, $name: String!, $remoteUrl: String!) {
       addRepo(projectId: $projectId, name: $name, remoteUrl: $remoteUrl) { id }
     }`,
    { projectId, name: repo.name, remoteUrl: repo.remoteUrl },
  );
  if (repo.baseBookmark !== "main") await setBaseBookmark(data.addRepo.id, repo.baseBookmark);
}

export async function setBaseBookmark(id: string, bookmark: string): Promise<void> {
  await gql(`mutation ($id: UUID!, $bookmark: String!) { updateRepo(id: $id, baseBookmark: $bookmark) { id } }`, {
    id,
    bookmark,
  });
}

export async function fetchRepo(id: string): Promise<void> {
  await gql(`mutation ($id: UUID!) { fetchRepo(id: $id) { id } }`, { id });
}

export async function loadWorkspaceMembers(workspaceId: string): Promise<WorkspaceMember[]> {
  const data = await gql<{ workspaceMembers: WorkspaceMember[] }>(
    `query ($id: UUID!) { workspaceMembers(workspaceId: $id) { userId name email role } }`,
    { id: workspaceId },
  );
  return data.workspaceMembers;
}

export async function loadWorkspaceIssues(workspaceId: string, includeDone = false): Promise<IssueSummary[]> {
  return loadBoard(workspaceId, includeDone);
}

/** The harness's own catalogue; the server reads it from the harness, not from us. */
export async function loadModels(harness: string): Promise<ModelOption[]> {
  const data = await gql<{ models: ModelOption[] }>(
    `query ($h: String!) { models(harness: $h) { id label provider costIn costOut contextWindow } }`,
    { h: harness },
  );
  return data.models;
}

/** One thing a person connects. Two entries can share a provider: Claude Code
 *  accepts a plan login or a billed key, and they are separate purchases. */
export type ConnectorRow = {
  id: string;
  provider: string;
  kind: string;
  group: string;
  name: string;
  icon: string;
  desc: string;
  enables: string;
  connected: boolean;
  label: string | null;
  appId: string | null;
  connectedAt: string | null;
  /** The harness adapters this entry can sign in (`claude_code`, `pi`); empty outside Agent access. */
  harnesses: string[];
  /** The plans a person can say they are on. Names only: the fee lives in Cost control. */
  plans: ConnectorPlan[];
  /** The chosen plan's id, if any. */
  plan: string | null;
};

export interface ConnectorPlan {
  id: string;
  name: string;
}

export const CONNECTION_FIELDS = `
  id provider kind group name icon desc enables connected label appId connectedAt harnesses plan plans { id name }
`;

export async function loadConnections(): Promise<ConnectorRow[]> {
  const data = await gql<{ connections: ConnectorRow[] }>(
    `query { connections { ${CONNECTION_FIELDS} } }`,
  );
  return data.connections;
}

/** Says which plan a connected entry is on, or clears it with null. */
export async function setConnectorPlan(id: string, plan: string | null): Promise<ConnectorRow> {
  const data = await gql<{ setConnectorPlan: ConnectorRow }>(
    `mutation ($id: String!, $plan: String) { setConnectorPlan(id: $id, plan: $plan) { ${CONNECTION_FIELDS} } }`,
    { id, plan },
  );
  return data.setConnectorPlan;
}

export type SignInState = "starting" | "waiting" | "done" | "failed" | "cancelled";

/** One OAuth sign-in the server drives through the harness's own login. */
export interface SignIn {
  id: string;
  connectorId: string;
  state: SignInState;
  /** The page to sign in on, once the login has printed it. */
  url: string | null;
  /** A device code to type on that page, when the provider uses one. */
  userCode: string | null;
  /** The provider shows a code after sign-in that has to be pasted back. */
  needsCode: boolean;
  error: string | null;
}

export const SIGN_IN_FIELDS = `id connectorId state url userCode needsCode error`;

export async function startSignIn(id: string): Promise<SignIn> {
  const data = await gql<{ startSignIn: SignIn }>(
    `mutation ($id: String!) { startSignIn(id: $id) { ${SIGN_IN_FIELDS} } }`,
    { id },
  );
  return data.startSignIn;
}

export async function loadSignIn(signInId: string): Promise<SignIn | null> {
  const data = await gql<{ signIn: SignIn | null }>(
    `query ($id: ID!) { signIn(signInId: $id) { ${SIGN_IN_FIELDS} } }`,
    { id: signInId },
  );
  return data.signIn;
}

export async function submitSignInCode(signInId: string, code: string): Promise<SignIn> {
  const data = await gql<{ submitSignInCode: SignIn }>(
    `mutation ($id: ID!, $code: String!) { submitSignInCode(signInId: $id, code: $code) { ${SIGN_IN_FIELDS} } }`,
    { id: signInId, code },
  );
  return data.submitSignInCode;
}

export async function cancelSignIn(signInId: string): Promise<boolean> {
  const data = await gql<{ cancelSignIn: boolean }>(
    `mutation ($id: ID!) { cancelSignIn(signInId: $id) }`,
    { id: signInId },
  );
  return data.cancelSignIn;
}

/* ------------------------------------------------------------ Pull requests --- */

/** The person's own GitHub account: what their token can do, never the token. */
export interface GithubAccount {
  login: string;
  name: string | null;
  avatarUrl: string | null;
  /** `github_pat_…a1B2`: enough to recognise the token, never enough to use it. */
  tokenHint: string;
  validatedAt: string;
  /** What probing found missing; empty when the token can do everything. */
  problems: string[];
}

export type PullRequestFilter = "OPEN" | "NEEDS_MY_REVIEW" | "MINE" | "BY_AGENTS" | "FAILING_CI" | "MERGED";
export type ReviewState = "DRAFT" | "REVIEW_REQUESTED" | "APPROVED" | "CHANGES_REQUESTED" | "MERGED" | "CLOSED";
export type CiState = "PASSING" | "FAILING" | "RUNNING" | "NONE";
/** GitHub's conflict check against the base branch; `UNKNOWN` until asked, or while GitHub computes it. */
export type MergeableState = "CLEAN" | "CONFLICTING" | "UNKNOWN";
export type MergeMethod = "MERGE" | "SQUASH" | "REBASE";
export type ReviewEvent = "APPROVE" | "REQUEST_CHANGES";
export type DiffSide = "LEFT" | "RIGHT";

export interface PullRequest {
  /** `<repoId>#<number>`. */
  id: string;
  repoId: string;
  /** `owner/repo`. */
  repoName: string;
  number: number;
  title: string;
  url: string;
  /** `open` or `closed`. */
  state: string;
  draft: boolean;
  merged: boolean;
  authorLogin: string;
  authorAvatarUrl: string | null;
  byAgent: boolean;
  headRef: string;
  headSha: string;
  baseRef: string;
  additions: number;
  deletions: number;
  changedFiles: number;
  reviewState: ReviewState;
  /** How many times a reviewer sent it back for changes; its review round is one more. */
  changesRequested: number;
  reviewRequestedFromMe: boolean;
  ciState: CiState;
  /** Whether it merges cleanly into `baseRef`; closed and merged ones are `UNKNOWN`. */
  mergeable: MergeableState;
  createdAt: string;
  updatedAt: string;
  mergedAt: string | null;
  /** The task the pull request carries, when Hulahoop opened it. */
  issue: { id: string; key: string; title: string; status: IssueStatus } | null;
  /** The task's queued or running run, whose working diff can be watched. */
  liveRun: { id: string; agentId: string; status: string } | null;
}

export interface PullRequestFile {
  path: string;
  previousPath: string | null;
  /** `added`, `removed`, `modified` or `renamed`. */
  status: string;
  additions: number;
  deletions: number;
  /** GitHub's hunks; null for a binary or too large file. */
  patch: string | null;
}

export interface PullRequestComment {
  id: string;
  path: string;
  line: number;
  side: DiffSide;
  body: string;
  authorLogin: string;
  createdAt: string;
  /** Saved in Hulahoop, not on GitHub yet: it goes with the next review. */
  pending: boolean;
  /** Pending, and there is no token to send it with. */
  onlyInHulahoop: boolean;
}

export interface PullRequestReview {
  authorLogin: string;
  state: string;
  body: string | null;
  submittedAt: string | null;
}

export interface CheckRun {
  name: string;
  status: string;
  conclusion: string | null;
  url: string | null;
}

export interface PullRequestDetail {
  pullRequest: PullRequest;
  body: string | null;
  files: PullRequestFile[];
  comments: PullRequestComment[];
  reviews: PullRequestReview[];
  checks: CheckRun[];
  mergeMethods: MergeMethod[];
  lastMergeMethod: MergeMethod | null;
  canReview: boolean;
  reviewBlockedReason: string | null;
  canMerge: boolean;
  mergeBlockedReason: string | null;
  /** Merged, and its branch in this repository is still on GitHub. */
  canDeleteBranch: boolean;
}

export interface RunDiff {
  runId: string;
  patch: string;
  sha: string;
  live: boolean;
  updatedAt: string;
}

export const GITHUB_ACCOUNT_FIELDS = `login name avatarUrl tokenHint validatedAt problems`;

export const PULL_REQUEST_FIELDS = `
  id repoId repoName number title url state draft merged authorLogin authorAvatarUrl byAgent
  headRef headSha baseRef additions deletions changedFiles reviewState changesRequested reviewRequestedFromMe ciState
  mergeable createdAt updatedAt mergedAt
  issue { id key title status }
  liveRun { id agentId status }
`;

export const PULL_REQUEST_COMMENT_FIELDS = `id path line side body authorLogin createdAt pending onlyInHulahoop`;

export const PULL_REQUEST_DETAIL_FIELDS = `
  pullRequest { ${PULL_REQUEST_FIELDS} }
  body
  files { path previousPath status additions deletions patch }
  comments { ${PULL_REQUEST_COMMENT_FIELDS} }
  reviews { authorLogin state body submittedAt }
  checks { name status conclusion url }
  mergeMethods lastMergeMethod canReview reviewBlockedReason canMerge mergeBlockedReason canDeleteBranch
`;

/** The person's GitHub account, or null before they add a token. */
export async function loadGithubAccount(): Promise<GithubAccount | null> {
  const data = await gql<{ githubAccount: GithubAccount | null }>(
    `query { githubAccount { ${GITHUB_ACCOUNT_FIELDS} } }`,
  );
  return data.githubAccount;
}

/** The server checks the token against GitHub before sealing it, and refuses a bad one. */
export async function setGithubToken(token: string): Promise<GithubAccount> {
  const data = await gql<{ setGithubToken: GithubAccount }>(
    `mutation ($token: String!) { setGithubToken(token: $token) { ${GITHUB_ACCOUNT_FIELDS} } }`,
    { token },
  );
  return data.setGithubToken;
}

export async function clearGithubToken(): Promise<boolean> {
  const data = await gql<{ clearGithubToken: boolean }>(`mutation { clearGithubToken }`);
  return data.clearGithubToken;
}

/** From the server's cache, which it keeps in sync with GitHub on its own. */
export async function loadPullRequests(filter: PullRequestFilter): Promise<PullRequest[]> {
  const data = await gql<{ pullRequests: PullRequest[] }>(
    `query ($filter: PullRequestFilter!) { pullRequests(filter: $filter) { ${PULL_REQUEST_FIELDS} } }`,
    { filter },
  );
  return data.pullRequests;
}

/** The sidebar's count. It reads the server's cache, so the 2.5 s poll may ask for it. */
export async function loadOpenPullRequestCount(): Promise<number> {
  const data = await gql<{ openPullRequestCount: number }>(`query { openPullRequestCount }`);
  return data.openPullRequestCount;
}

export interface PullRequestSyncStatus {
  syncedAt: string | null;
  problems: string[];
}

export async function loadPullRequestSyncStatus(): Promise<PullRequestSyncStatus> {
  const data = await gql<{ pullRequestsSyncedAt: string | null; pullRequestSyncProblems: string[] }>(
    `query { pullRequestsSyncedAt pullRequestSyncProblems }`,
  );
  return { syncedAt: data.pullRequestsSyncedAt, problems: data.pullRequestSyncProblems };
}

/** Files, reviews, comments and checks, which the server reads from GitHub. */
export async function loadPullRequest(repoId: string, number: number): Promise<PullRequestDetail | null> {
  const data = await gql<{ pullRequest: PullRequestDetail | null }>(
    `query ($repoId: UUID!, $number: Int!) { pullRequest(repoId: $repoId, number: $number) { ${PULL_REQUEST_DETAIL_FIELDS} } }`,
    { repoId, number },
  );
  return data.pullRequest;
}

/** Ask the server to sync now; returns when it finished. */
export async function syncPullRequests(): Promise<string> {
  const data = await gql<{ syncPullRequests: string }>(`mutation { syncPullRequests }`);
  return data.syncPullRequests;
}

export interface PullRequestCommentInput {
  repoId: string;
  number: number;
  path: string;
  line: number;
  side: DiffSide;
  body: string;
}

export async function addPullRequestComment(input: PullRequestCommentInput): Promise<PullRequestComment> {
  const data = await gql<{ addPullRequestComment: PullRequestComment }>(
    `mutation ($repoId: UUID!, $number: Int!, $path: String!, $line: Int!, $side: DiffSide!, $body: String!) {
      addPullRequestComment(repoId: $repoId, number: $number, path: $path, line: $line, side: $side, body: $body) { ${PULL_REQUEST_COMMENT_FIELDS} }
    }`,
    { ...input },
  );
  return data.addPullRequestComment;
}

/** Only a pending comment can go; one on GitHub stays there. */
export async function deletePullRequestComment(id: string): Promise<boolean> {
  const data = await gql<{ deletePullRequestComment: boolean }>(
    `mutation ($id: ID!) { deletePullRequestComment(id: $id) }`,
    { id },
  );
  return data.deletePullRequestComment;
}

/** Sends the review with every pending line comment. `body` may be empty when comments carry it. */
export async function reviewPullRequest(
  repoId: string,
  number: number,
  event: ReviewEvent,
  body: string | null,
): Promise<PullRequestDetail> {
  const data = await gql<{ reviewPullRequest: PullRequestDetail }>(
    `mutation ($repoId: UUID!, $number: Int!, $event: ReviewEvent!, $body: String) {
      reviewPullRequest(repoId: $repoId, number: $number, event: $event, body: $body) { ${PULL_REQUEST_DETAIL_FIELDS} }
    }`,
    { repoId, number, event, body },
  );
  return data.reviewPullRequest;
}

/** `headSha` pins the merge to the head the person saw: GitHub refuses it if someone pushed since. */
export async function mergePullRequest(
  repoId: string,
  number: number,
  method: MergeMethod,
  headSha?: string | null,
): Promise<PullRequest> {
  const data = await gql<{ mergePullRequest: PullRequest }>(
    `mutation ($repoId: UUID!, $number: Int!, $method: MergeMethod!, $headSha: String) {
      mergePullRequest(repoId: $repoId, number: $number, method: $method, headSha: $headSha) { ${PULL_REQUEST_FIELDS} }
    }`,
    { repoId, number, method, headSha: headSha ?? null },
  );
  return data.mergePullRequest;
}

/** Deletes the branch a merged pull request left on GitHub. */
export async function deletePullRequestBranch(repoId: string, number: number): Promise<PullRequestDetail> {
  const data = await gql<{ deletePullRequestBranch: PullRequestDetail }>(
    `mutation ($repoId: UUID!, $number: Int!) {
      deletePullRequestBranch(repoId: $repoId, number: $number) { ${PULL_REQUEST_DETAIL_FIELDS} }
    }`,
    { repoId, number },
  );
  return data.deletePullRequestBranch;
}

// --- people: sign-in, members, registration, personal tokens ---------------

export type Role = "owner" | "member";
export type RegistrationMode = "closed" | "invite" | "open";

export interface Me {
  id: string;
  name: string;
  email: string | null;
  role: Role;
}

export interface Member {
  userId: string;
  name: string;
  email: string | null;
  role: Role;
}

export interface ApiToken {
  id: string;
  name: string;
  createdAt: string;
}

/** Posts to one of the server's public `/auth/*` routes and returns the token it answers with. */
async function authRequest(server: string, path: string, body: Record<string, unknown>): Promise<string> {
  const url = normalizeServer(server);
  let response: Response;
  try {
    response = await fetch(`${url}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error(`could not reach ${url}. Is the server running, and does it listen on that address?`);
  }
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(
      response.status === 401 ? "wrong email or password" : (payload.error ?? `${url} answered ${response.status}`),
    );
  }
  return payload.token as string;
}

/** A session token is a bearer like any other: it is saved as the connection's token. */
export async function signIn(server: string, email: string, password: string): Promise<void> {
  const token = await authRequest(server, "/auth/login", { email, password });
  await probe(server, token);
  saveRemote(server, token);
}

/** Registration is open, or the person holds an invite from an owner. */
export async function signUp(
  server: string,
  person: { name: string; email: string; password: string; invite?: string },
): Promise<void> {
  const token = await authRequest(server, "/auth/signup", { ...person, invite: person.invite || null });
  await probe(server, token);
  saveRemote(server, token);
}

/** Forgets this device's session on the server, then the saved connection. */
export async function signOut(): Promise<void> {
  const { url, token } = await connect();
  await fetch(`${url}/auth/logout`, { method: "POST", headers: { authorization: `Bearer ${token}` } }).catch(
    () => undefined,
  );
  useLocalServer();
}

export async function loadMe(): Promise<Me> {
  return (await gql<{ me: Me }>(`query { me { id name email role } }`)).me;
}

export async function loadRegistration(): Promise<RegistrationMode> {
  return (await gql<{ registration: RegistrationMode }>(`query { registration }`)).registration;
}

export async function setRegistration(mode: RegistrationMode): Promise<void> {
  await gql(`mutation ($mode: String!) { setRegistration(mode: $mode) }`, { mode });
}

export async function loadMembers(workspaceId: string): Promise<Member[]> {
  const data = await gql<{ workspaceMembers: Member[] }>(
    `query ($workspaceId: UUID!) { workspaceMembers(workspaceId: $workspaceId) { userId name email role } }`,
    { workspaceId },
  );
  return data.workspaceMembers;
}

export async function setMemberRole(workspaceId: string, userId: string, role: Role): Promise<void> {
  await gql(
    `mutation ($workspaceId: UUID!, $userId: UUID!, $role: String!) { setMemberRole(workspaceId: $workspaceId, userId: $userId, role: $role) }`,
    { workspaceId, userId, role },
  );
}

export async function removeMember(workspaceId: string, userId: string): Promise<void> {
  await gql(`mutation ($workspaceId: UUID!, $userId: UUID!) { removeMember(workspaceId: $workspaceId, userId: $userId) }`, {
    workspaceId,
    userId,
  });
}

/** A single-use invite code; whoever enters it on the sign-up form joins with `role`. */
export async function createInvite(role: Role): Promise<string> {
  const { url, token } = await connect();
  const response = await fetch(`${url}/auth/invites`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ role }),
  });
  const payload = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(payload.error ?? `the server answered ${response.status}`);
  return payload.token as string;
}

export async function loadApiTokens(): Promise<ApiToken[]> {
  return (await gql<{ apiTokens: ApiToken[] }>(`query { apiTokens { id name createdAt } }`)).apiTokens;
}

/** The token's value comes back once; only its hash is kept. */
export async function createApiToken(name: string): Promise<string> {
  const data = await gql<{ createApiToken: { token: string } }>(
    `mutation ($name: String!) { createApiToken(name: $name) { id token } }`,
    { name },
  );
  return data.createApiToken.token;
}

export async function revokeApiToken(id: string): Promise<void> {
  await gql(`mutation ($id: UUID!) { revokeApiToken(id: $id) }`, { id });
}
