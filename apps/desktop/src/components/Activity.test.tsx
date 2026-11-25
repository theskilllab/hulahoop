import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Comment, Run } from "../api";

vi.mock("../api", () => ({ addComment: vi.fn(), gql: vi.fn() }));

import { CommentItem, RunCard } from "./Activity";

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

// jsdom has no layout: pretend every text box is far taller than the clamp.
beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(1000);
  const computed = window.getComputedStyle.bind(window);
  vi.spyOn(window, "getComputedStyle").mockImplementation((el) => {
    const style = computed(el);
    return new Proxy(style, { get: (target, key) => (key === "fontSize" ? "16px" : Reflect.get(target, key)) });
  });
});

const comment = (over: Partial<Comment> = {}): Comment => ({
  id: "c1",
  author: "Developer",
  authorType: "agent",
  body: "Done.",
  pinned: false,
  createdAt: "2026-01-01T00:00:00Z",
  ...over,
});

describe("comments never carry an Open transcript link", () => {
  it("even an agent comment that came from a run", () => {
    render(<CommentItem comment={comment({ runId: "r1" })} mentions={[]} />);
    expect(screen.queryByText("Open transcript")).toBeNull();
  });
});

describe("agent runs fold long summaries", () => {
  it("shows Show more on a long run summary and expands it", () => {
    render(<RunCard run={run({ summary: "line\n\n".repeat(40) })} agentName="Reviewer" onOpenTranscript={vi.fn()} />);
    const more = screen.getByRole("button", { name: "Show more" });
    fireEvent.click(more);
    expect(screen.getByRole("button", { name: "Show less" })).toBeTruthy();
  });

  it("leaves a short run summary unfolded", () => {
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(10);
    render(<RunCard run={run({ summary: "Short." })} agentName="Reviewer" onOpenTranscript={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
  });

  it("keeps the run's own Open transcript button", () => {
    render(<RunCard run={run()} agentName="Reviewer" onOpenTranscript={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Open transcript" })).toBeTruthy();
  });
});
