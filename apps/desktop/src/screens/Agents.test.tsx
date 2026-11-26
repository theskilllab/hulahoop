import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Agent, ConnectorRow, IssueSummary, ModelOption, RecentRun } from "../api";

const api = vi.hoisted(() => ({
  gql: vi.fn(),
  loadConnections: vi.fn(),
  loadModels: vi.fn(),
  setAgentInstructions: vi.fn(),
  setAgentSkills: vi.fn(),
}));

vi.mock("../api", () => api);

vi.mock("../ws", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../ws")>()),
  useRunDiff: () => ({ patch: "", live: false, updatedAt: null, error: null, reconnecting: false }),
}));

import Agents, { agentState, latestRunFor } from "./Agents";

// jsdom has <dialog> but not its modal methods.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
    this.dispatchEvent(new Event("close"));
  };
});

const run = (over: Partial<RecentRun> = {}): RecentRun => ({
  id: "r1",
  issueId: "i1",
  agentId: "a1",
  status: "succeeded",
  adapter: "pi",
  model: "m",
  attempt: 1,
  tokensIn: 0,
  tokensOut: 0,
  error: null,
  startedAt: "2025-01-01T10:00:00Z",
  finishedAt: null,
  headBookmark: null,
  ...over,
});

const issue = (over: Partial<IssueSummary> = {}) =>
  ({ id: "i1", key: "HUL-1", title: "First task", status: "in_progress", blockedBy: null, ...over }) as IssueSummary;

const entry = (id: string, name: string, harnesses: string[], connected = true): ConnectorRow => ({
  id,
  provider: id,
  kind: "subscription",
  group: harnesses.length ? "Agent access" : "Source control",
  name,
  icon: "spark",
  desc: "",
  enables: "",
  connected,
  label: null,
  appId: null,
  connectedAt: null,
  harnesses,
  plans: [],
  plan: null,
});

const connections = (): ConnectorRow[] => [
  entry("github", "GitHub", []),
  entry("claude-sub", "Claude subscription", ["claude_code", "pi"]),
  entry("claude-api", "Claude API key", ["claude_code", "pi"]),
  entry("codex-sub", "Codex subscription", ["pi"]),
  entry("opencode-go", "OpenCode Go key", ["pi"], false),
  entry("pi", "Pi login", ["pi"]),
];

const model = (id: string): ModelOption => ({
  id,
  label: id.split("/")[1],
  provider: id.split("/")[0],
  costIn: null,
  costOut: null,
  contextWindow: null,
});

const PI_MODELS = [model("anthropic/claude-sonnet-4"), model("openai-codex/gpt-5"), model("opencode-go/kimi-k2")];

const agent = (over: Partial<Agent> = {}): Agent => ({
  id: "a1",
  name: "Builder",
  adapter: "pi",
  model: "anthropic/claude-sonnet-4",
  connector: null,
  agentsMd: "",
  skills: [],
  ...over,
});

beforeEach(() => {
  api.gql.mockImplementation(async (query: string) => (query.includes("connections") ? { connections: connections() } : {}));
  api.loadConnections.mockImplementation(async () => connections());
  api.loadModels.mockImplementation(async (harness: string) => (harness === "pi" ? PI_MODELS : [model("claude/sonnet")]));
});

function open(agents: Agent[] = [agent()], extra: { issues?: IssueSummary[]; runs?: RecentRun[] } = {}) {
  render(
    <Agents
      workspaceId="w1"
      agents={agents}
      skills={[]}
      issues={extra.issues ?? []}
      runs={extra.runs ?? []}
      repoId={null}
      projectId={null}
      onChanged={vi.fn()}
      onOpenSkills={vi.fn()}
      onOpenIssue={vi.fn()}
      onOpenConnectors={vi.fn()}
    />,
  );
}

async function editAgent() {
  await userEvent.click(await screen.findByRole("button", { name: /Edit agent/ }));
  return screen.getByRole("dialog");
}

/** Open a shared dropdown by its label and return its listbox. */
async function openSelect(dialog: HTMLElement, label: string) {
  await userEvent.click(await within(dialog).findByRole("button", { name: new RegExp(label) }));
  return document.querySelector(".sel-pop") as HTMLElement;
}
const optionsOf = (pop: HTMLElement) =>
  within(pop)
    .getAllByRole("option")
    .map((option) => (option.getAttribute("aria-disabled") === "true" ? `${option.textContent} [disabled]` : option.textContent));
const valueOf = (dialog: HTMLElement, label: string) =>
  within(dialog).getByRole("button", { name: new RegExp(label) }).textContent;

describe("status derivation", () => {
  it("latestRunFor prefers a running run, else the newest", () => {
    const old = run({ id: "old", startedAt: "2025-01-01T00:00:00Z" });
    const newer = run({ id: "new", startedAt: "2025-01-02T00:00:00Z" });
    const live = run({ id: "live", status: "running", startedAt: "2025-01-01T12:00:00Z" });
    expect(latestRunFor("a1", [old, newer])?.id).toBe("new");
    expect(latestRunFor("a1", [old, newer, live])?.id).toBe("live");
    expect(latestRunFor("a1", [run({ agentId: "other" })])).toBeNull();
  });

  it("agentState is running, blocked or idle from real runs and issues", () => {
    expect(agentState("a1", [run({ status: "running" })], [issue()])).toBe("running");
    const blocked = issue({ status: "blocked", blockedBy: "Which colour?" });
    expect(agentState("a1", [run()], [blocked])).toBe("blocked");
    // someone else ran after this agent: not waiting on its question
    expect(agentState("a1", [run(), run({ id: "r2", agentId: "a2", startedAt: "2025-02-01T00:00:00Z" })], [blocked])).toBe("idle");
    expect(agentState("a1", [run()], [issue()])).toBe("idle");
    expect(agentState("a1", [], [])).toBe("idle");
  });
});

describe("roster", () => {
  it("shows each card's own task, harness line and status chip", async () => {
    open([agent(), agent({ id: "a2", name: "Reviewer", adapter: "claude_code", model: "" })], {
      issues: [issue(), issue({ id: "i2", key: "HUL-2", title: "Second task" })],
      runs: [run(), run({ id: "r2", agentId: "a2", issueId: "i2", status: "running" })],
    });
    const cards = await screen.findAllByRole("button", { name: /Builder|Reviewer/ });
    const builder = cards.find((c) => c.textContent?.includes("Builder") && c.className.includes("acard")) as HTMLElement;
    const reviewer = cards.find((c) => c.textContent?.includes("Reviewer") && c.className.includes("acard")) as HTMLElement;
    expect(builder.textContent).toContain("HUL-1 · First task");
    expect(builder.textContent).not.toContain("Second task");
    expect(builder.textContent).toContain("Idle");
    expect(reviewer.textContent).toContain("HUL-2 · Second task");
    expect(reviewer.textContent).toContain("Running");
  });

  it("says No task assigned and shows the empty roster text", async () => {
    open();
    expect((await screen.findByText("No task assigned")).closest(".acard")).toBeTruthy();
  });

  it("shows the empty roster", async () => {
    open([]);
    expect(await screen.findByText("No agents yet")).toBeTruthy();
    expect(screen.getByText("Create one, then give it a harness that has a connection.")).toBeTruthy();
  });

  it("subtitle counts running agents", async () => {
    open([agent()], { runs: [run({ status: "running" })], issues: [issue()] });
    expect(await screen.findByText("1 running · Local runner")).toBeTruthy();
  });
});

describe("agent pane", () => {
  it("shows the blocked question and an Answer button", async () => {
    open([agent()], { runs: [run()], issues: [issue({ status: "blocked", blockedBy: "Which colour?" })] });
    expect(await screen.findByText("Which colour?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Answer on HUL-1" })).toBeTruthy();
  });

  it("shows empty states on the tabs", async () => {
    open();
    expect(await screen.findByText("No changes yet")).toBeTruthy();
    await userEvent.click(screen.getByRole("tab", { name: /Transcript/ }));
    expect(screen.getByText("No active session")).toBeTruthy();
  });

  it("asks before stopping a running run", async () => {
    open([agent()], { runs: [run({ status: "running" })], issues: [issue()] });
    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));
    expect(api.gql).not.toHaveBeenCalledWith(expect.stringContaining("cancelRun"), expect.anything());
    await userEvent.click(screen.getByRole("button", { name: "Stop run" }));
    expect(api.gql).toHaveBeenCalledWith(expect.stringContaining("cancelRun"), expect.objectContaining({ runId: "r1" }));
  });
});

describe("agent dialog", () => {
  it("is a real dialog with the prototype's headings and no native select", async () => {
    open();
    const dialog = await editAgent();
    expect(dialog.tagName).toBe("DIALOG");
    expect(within(dialog).getByRole("heading", { name: "Edit agent" })).toBeTruthy();
    expect(document.querySelector("select")).toBeNull();
  });

  it("rejects an empty name inline", async () => {
    open();
    await userEvent.click(await screen.findByRole("button", { name: /New agent/ }));
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create agent" }));
    expect(within(dialog).getByText("Give the agent a name.")).toBeTruthy();
    expect(within(dialog).getByLabelText(/Name/).getAttribute("aria-invalid")).toBe("true");
  });

  it("rejects a duplicate name, ignoring case", async () => {
    open();
    await userEvent.click(await screen.findByRole("button", { name: /New agent/ }));
    const dialog = screen.getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText(/Name/), "builder");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create agent" }));
    expect(within(dialog).getByText('An agent named "builder" already exists.')).toBeTruthy();
  });

  it("confirms a delete inline, without confirm()", async () => {
    const confirm = vi.spyOn(window, "confirm");
    open();
    const dialog = await editAgent();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete agent" }));
    expect(within(dialog).getByText("Delete Builder? Runs it started stay in the history.")).toBeTruthy();
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel", hidden: false }));
    expect(within(dialog).queryByText(/Runs it started stay in the history/)).toBeNull();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete agent" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(api.gql).toHaveBeenCalledWith(expect.stringContaining("deleteAgent"), expect.anything());
    expect(confirm).not.toHaveBeenCalled();
  });

  it("lists skills as toggle chips and Manage skills closes the dialog", async () => {
    const onOpenSkills = vi.fn();
    render(
      <Agents workspaceId="w1" agents={[agent()]} skills={[{ id: "s1", name: "Testing", description: "Writes tests" } as never]} issues={[]} runs={[]} repoId={null} projectId={null} onChanged={vi.fn()} onOpenSkills={onOpenSkills} onOpenIssue={vi.fn()} onOpenConnectors={vi.fn()} />,
    );
    const dialog = await editAgent();
    const chip = within(dialog).getByRole("button", { name: "Testing" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    await userEvent.click(chip);
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    await userEvent.click(within(dialog).getByRole("button", { name: "Manage skills" }));
    expect(onOpenSkills).toHaveBeenCalled();
  });
});

describe("Pi agent connector", () => {
  it("offers Automatic, the connected entries for the model's provider, and the Pi login", async () => {
    open();
    const dialog = await editAgent();
    await waitFor(async () => {
      const pop = await openSelect(dialog, "Signs in with");
      expect(optionsOf(pop)).toEqual(["Automatic", "Claude subscription", "Claude API key", "Pi login"]);
    });
    expect(valueOf(dialog, "Signs in with")).toContain("Automatic");
  });

  it("follows the model to another provider and lets go of a choice that no longer applies", async () => {
    open();
    const dialog = await editAgent();
    await userEvent.click(await within(await (async () => { await waitFor(() => expect(within(dialog).getByRole("button", { name: /Signs in with/ })).toBeTruthy()); return await openSelect(dialog, "Signs in with"); })()).findByRole("option", { name: /Claude subscription/ }));
    expect(valueOf(dialog, "Signs in with")).toContain("Claude subscription");

    await userEvent.click(await within(await openSelect(dialog, "Model")).findByRole("option", { name: /gpt-5/ }));
    const pop = await openSelect(dialog, "Signs in with");
    expect(optionsOf(pop)).toEqual(["Automatic", "Codex subscription", "Pi login"]);
    await userEvent.keyboard("{Escape}");
    expect(valueOf(dialog, "Signs in with")).toContain("Automatic");
  });

  it("shows a stored entry that is not connected, disabled", async () => {
    open([agent({ model: "opencode-go/kimi-k2", connector: "opencode-go" })]);
    const dialog = await editAgent();
    await waitFor(async () => {
      const pop = await openSelect(dialog, "Signs in with");
      expect(optionsOf(pop)).toEqual(["Automatic", "Pi login", "OpenCode Go key (not connected) [disabled]"]);
      await userEvent.keyboard("{Escape}");
    });
  });

  it("groups models by provider and lists unconnected harnesses as disabled", async () => {
    open();
    const dialog = await editAgent();
    const pop = await openSelect(dialog, "Model");
    expect(within(pop).getByText("anthropic")).toBeTruthy();
    expect(within(pop).getByPlaceholderText("Search…")).toBeTruthy();
    await userEvent.keyboard("{Escape}");
    const harness = await openSelect(dialog, "Harness");
    expect(within(harness).getByRole("option", { name: /Pi/ })).toBeTruthy();
  });
});
