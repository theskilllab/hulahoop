import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { gql, type Agent, type Comment, type Issue, type Run } from "../api";

const room = vi.hoisted(() => ({
  running: 1,
  waiting: 1,
  freeMemoryBytes: 1_100_000_000,
  freeDiskBytes: 0,
  freeCpuCores: 0,
  maxRuns: 4,
  reportedAt: "2026-01-01T00:00:00Z",
}));
vi.mock("../api", () => ({ addComment: vi.fn(), gql: vi.fn(), loadRunnerCapacity: vi.fn(async () => room) }));

import { publishRunnerCapacity } from "../runnerCapacity";
import Activity, { CommentItem, RunCard } from "./Activity";

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

  it("shows the failure reason, with the agent's last output folded away", () => {
    render(
      <RunCard
        run={run({ status: "failed", error: "the agent exited with Some(1).\n{\"type\":\"delta\"}" })}
        agentName="Reviewer"
        onOpenTranscript={vi.fn()}
      />,
    );
    expect((document.querySelector(".run-why") as HTMLElement).textContent).toBe("the agent exited with Some(1).");
    expect((document.querySelector(".run-tail pre") as HTMLElement).textContent).toBe('{"type":"delta"}');
  });

  it("names the host login too", () => {
    render(<RunCard run={run({ credentialSource: "host login" })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.getByText(/Credential: host login/)).toBeTruthy();
  });

  it("leaves the line out for runs recorded before it existed", () => {
    render(<RunCard run={run({ credentialSource: null })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.queryByText(/Credential:/)).toBeNull();
  });

  it("says why a queued run has not started, and nothing once it runs", () => {
    const reason = "Waiting for memory: needs ~2.4 GB, 1.1 GB free";
    publishRunnerCapacity(room);
    render(<RunCard run={run({ status: "queued", waitReason: reason })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.getByText(reason)).toBeTruthy();
    cleanup();
    render(<RunCard run={run({ status: "running", waitReason: reason })} agentName="Builder" onOpenTranscript={vi.fn()} />);
    expect(screen.queryByText(reason)).toBeNull();
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

describe("running an agent again", () => {
  const agents = [{ id: "a1", name: "Reviewer" }] as Agent[];
  const task = (runs: Run[], over: Partial<Issue> = {}) =>
    ({ id: "i1", status: "in_progress", runs, comments: [], runsSinceCap: runs.length, runLimit: 12, ...over }) as unknown as Issue;
  const show = (issue: Issue, onChanged = vi.fn()) =>
    render(
      <Activity issue={issue} agents={agents} draft="" onDraft={vi.fn()} onOpenTranscript={vi.fn()} onChanged={onChanged} onError={vi.fn()} />,
    );

  it("starts the same agent from the same work when the newest run timed out", async () => {
    const onChanged = vi.fn();
    vi.mocked(gql).mockResolvedValue({ startRun: { id: "r3" } });
    show(
      task([
        run({ id: "r1", status: "succeeded", createdAt: "2026-01-01T00:00:00Z" }),
        run({ id: "r2", status: "timed_out", parentRunId: "r1", createdAt: "2026-01-01T01:00:00Z" }),
      ]),
      onChanged,
    );
    const again = screen.getAllByRole("button", { name: "Run Reviewer again" });
    expect(again).toHaveLength(1);
    fireEvent.click(again[0]);
    await vi.waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(vi.mocked(gql).mock.calls.at(-1)?.[1]).toEqual({ issue: "i1", agent: "a1", parent: "r1" });
  });

  it("is not offered after a success, while a run is live, or at the run limit", () => {
    show(task([run({ id: "r1", status: "succeeded", createdAt: "2026-01-01T00:00:00Z" })]));
    expect(screen.queryByRole("button", { name: "Run Reviewer again" })).toBeNull();
    cleanup();
    show(
      task([
        run({ id: "r1", status: "failed", createdAt: "2026-01-01T00:00:00Z" }),
        run({ id: "r2", status: "running", createdAt: "2026-01-01T01:00:00Z" }),
      ]),
    );
    expect(screen.queryByRole("button", { name: "Run Reviewer again" })).toBeNull();
    cleanup();
    show(task([run({ id: "r1", status: "failed", createdAt: "2026-01-01T00:00:00Z" })], { runsSinceCap: 12 }));
    expect(screen.queryByRole("button", { name: "Run Reviewer again" })).toBeNull();
  });
});
