import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Run } from "../api";

vi.mock("../api", () => ({ addComment: vi.fn(), gql: vi.fn() }));

import { RunCard } from "./Activity";

const run = (over: Partial<Run> = {}): Run => ({
  id: "r1",
  agentId: "a1",
  status: "succeeded",
  attempt: 1,
  adapter: "pi",
  flags: [],
  headBookmark: null,
  headRevision: null,
  diffSha: null,
  commitName: null,
  commitEmail: null,
  prUrl: null,
  prNumber: null,
  blockedBy: null,
  projectName: null,
  error: null,
  tokensIn: 0,
  tokensOut: 0,
  startedAt: "2026-01-01T00:00:00Z",
  finishedAt: "2026-01-01T00:01:00Z",
  model: "anthropic/claude-sonnet-4",
  ...over,
});

describe("RunCard", () => {
  it("says which credential the run started with, next to the model", () => {
    render(<RunCard run={run({ credentialSource: "Claude subscription" })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    const meta = document.querySelector(".run-meta") as HTMLElement;
    expect(meta.textContent).toContain("anthropic/claude-sonnet-4");
    expect(meta.textContent).toContain("Credential: Claude subscription");
  });

  it("names the host login too", () => {
    render(<RunCard run={run({ credentialSource: "host login" })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.getByText(/Credential: host login/)).toBeTruthy();
  });

  it("leaves the line out for runs recorded before it existed", () => {
    render(<RunCard run={run({ credentialSource: null })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.queryByText(/Credential:/)).toBeNull();
  });
});
