import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Agent, Issue, Milestone, Project } from "../api";

const api = vi.hoisted(() => ({ gql: vi.fn(), updateIssue: vi.fn(), moveIssue: vi.fn() }));

vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api")>()), ...api }));

import IssueDetail from "./IssueDetail";

const issue = (over: Partial<Issue> = {}) =>
  ({
    id: "i1",
    key: "HUL-1",
    number: 1,
    projectId: "p1",
    projectPrefix: "HUL",
    milestoneId: null,
    title: "A task",
    body: "",
    status: "todo",
    priority: "medium",
    dueDate: null,
    assignee: null,
    attempt: 0,
    runCount: 0,
    mergeRevision: null,
    prUrl: null,
    prNumber: null,
    pinnedCommentId: null,
    blockedBy: null,
    projectName: "Hulahoop",
    moves: ["in_progress"],
    files: [],
    runs: [],
    reviews: [],
    comments: [],
    currentRun: null,
    ...over,
  }) as unknown as Issue;

const projects = [
  { id: "p1", name: "Hulahoop", prefix: "HUL" },
  { id: "p2", name: "Other", prefix: "OTH" },
] as unknown as Project[];
const milestones = [{ id: "m1", projectId: "p1", title: "0.1.0", status: "open" }] as unknown as Milestone[];

function setup(value: Issue = issue()) {
  const onChanged = vi.fn();
  const onClose = vi.fn();
  render(
    <IssueDetail
      issue={value}
      draft={{}}
      onDraft={() => {}}
      workspaceId="w1"
      agents={[] as Agent[]}
      projects={projects}
      milestones={milestones}
      columns={[]}
      artifacts={[]}
      onNewArtifact={() => {}}
      onOpenArtifact={() => {}}
      onChanged={onChanged}
      onOpenTranscript={() => {}}
      onSaved={() => {}}
      onError={() => {}}
    />,
  );
  return { onChanged, onClose };
}

beforeEach(() => {
  api.gql.mockReset().mockResolvedValue({});
  api.updateIssue.mockReset().mockResolvedValue(undefined);
  api.moveIssue.mockReset().mockResolvedValue(undefined);
});

describe("IssueDetail properties use the shared dropdown", () => {
  it("has no native select", () => {
    setup();
    expect(document.querySelector("select")).toBeNull();
    for (const name of ["Status", "Priority", "Assignee", "Project", "Milestone"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
  });

  it("opens Priority as a popover with a check on the current value and saves the pick", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(document.querySelector(".sel-pop")).not.toBeNull();
    const current = screen.getAllByRole("option").filter((option) => option.getAttribute("aria-selected") === "true");
    expect(current).toHaveLength(1);
    expect(current[0].textContent).toMatch(/medium/i);
    await userEvent.click(screen.getByRole("option", { name: /high/i }));
    expect(api.updateIssue).toHaveBeenCalledWith("i1", { priority: "high" });
  });

  it("moves status through the popover", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: "Status" }));
    await userEvent.click(screen.getByRole("option", { name: /in progress/i }));
    expect(api.moveIssue).toHaveBeenCalledWith("i1", "in_progress");
  });

  it("cannot open Status when the workflow declares no moves", async () => {
    setup(issue({ moves: [] }));
    await userEvent.click(screen.getByRole("button", { name: "Status" }));
    expect(document.querySelector(".sel-pop")).toBeNull();
  });

  it("saves No project as a null project", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: "Project" }));
    await userEvent.click(screen.getByRole("option", { name: "No project" }));
    expect(api.updateIssue).toHaveBeenCalledWith("i1", { projectId: null });
  });

  it("Escape closes the popover without saving and refocuses the control", async () => {
    setup();
    const trigger = screen.getByRole("button", { name: "Priority" });
    await userEvent.click(trigger);
    await userEvent.keyboard("{Escape}");
    expect(document.querySelector(".sel-pop")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(api.updateIssue).not.toHaveBeenCalled();
  });
});
