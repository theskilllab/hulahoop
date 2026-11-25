import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Agent, ConnectorRow, ModelOption } from "../api";

const api = vi.hoisted(() => ({
  gql: vi.fn(),
  loadConnections: vi.fn(),
  loadModels: vi.fn(),
  setAgentInstructions: vi.fn(),
  setAgentSkills: vi.fn(),
}));

vi.mock("../api", () => api);

import Agents from "./Agents";

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

function open(agents: Agent[] = [agent()]) {
  render(
    <Agents
      workspaceId="w1"
      agents={agents}
      skills={[]}
      issues={[]}
      runs={[]}
      repoId={null}
      projectId={null}
      onChanged={vi.fn()}
      onOpenSkills={vi.fn()}
    />,
  );
}

async function editAgent() {
  await userEvent.click(await screen.findByRole("button", { name: /Edit agent/ }));
  return screen.getByRole("dialog");
}

const optionsOf = (select: HTMLElement) =>
  within(select)
    .getAllByRole("option")
    .map((option) => ((option as HTMLOptionElement).disabled ? `${option.textContent} [disabled]` : option.textContent));

describe("Pi agent connector", () => {
  it("offers Automatic, the connected entries for the model's provider, and the Pi login", async () => {
    open();
    const dialog = await editAgent();
    const select = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    await waitFor(() =>
      expect(optionsOf(select)).toEqual(["Automatic", "Claude subscription", "Claude API key", "Pi login"]),
    );
    expect(select.value).toBe("");
  });

  it("follows the model to another provider and lets go of a choice that no longer applies", async () => {
    open();
    const dialog = await editAgent();
    const select = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    await waitFor(() => expect(optionsOf(select)).toContain("Claude subscription"));
    await userEvent.selectOptions(select, "claude-sub");
    expect(select.value).toBe("claude-sub");

    await userEvent.selectOptions(within(dialog).getByLabelText("Model"), "openai-codex/gpt-5");
    expect(optionsOf(select)).toEqual(["Automatic", "Codex subscription", "Pi login"]);
    expect(select.value).toBe("");
  });

  it("keeps the Pi login across a model change", async () => {
    open();
    const dialog = await editAgent();
    const select = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    await waitFor(() => expect(optionsOf(select)).toContain("Pi login"));
    await userEvent.selectOptions(select, "pi");
    await userEvent.selectOptions(within(dialog).getByLabelText("Model"), "opencode-go/kimi-k2");
    expect(select.value).toBe("pi");
    // OpenCode Go has no connected entry, so only the Pi login is offered.
    expect(optionsOf(select)).toEqual(["Automatic", "Pi login"]);
  });

  it("shows a stored entry that is not connected, disabled, instead of Automatic", async () => {
    open([agent({ model: "opencode-go/kimi-k2", connector: "opencode-go" })]);
    const dialog = await editAgent();
    const select = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    await waitFor(() =>
      expect(optionsOf(select)).toEqual(["Automatic", "Pi login", "OpenCode Go key (not connected) [disabled]"]),
    );
    expect(select.value).toBe("opencode-go");
  });

  it("clears a Pi-only choice when the harness changes to Claude Code", async () => {
    open([agent({ connector: "pi" })]);
    const dialog = await editAgent();
    const select = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe("pi"));
    await userEvent.selectOptions(within(dialog).getByLabelText("Harness"), "claude_code");
    const claude = (await within(dialog).findByLabelText("Signs in with")) as HTMLSelectElement;
    expect(optionsOf(claude)).toEqual(["Automatic", "Claude subscription", "Claude API key"]);
    expect(claude.value).toBe("");
  });
});
