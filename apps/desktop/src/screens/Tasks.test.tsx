import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IssueSummary } from "../api";

vi.mock("../api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api")>()),
  gql: vi.fn().mockResolvedValue({}),
}));

import Tasks from "./Tasks";

const issue = (key: string, projectId: string | null): IssueSummary =>
  ({ id: key, key, title: `Title ${key}`, status: "todo", priority: "medium", projectId, labels: [] }) as unknown as IssueSummary;

const issues = [issue("HUL-1", "p1"), issue("HUL-2", null)];

function setup(projectFilter: string, onProjectFilter = vi.fn()) {
  localStorage.setItem("hulahoop.tasks.layout", "list");
  render(
    <Tasks
      workspaceId="w1"
      issues={issues}
      projects={[{ id: "p1", name: "Hulahoop", prefix: "HUL" } as never]}
      agents={[]}
      milestones={[]}
      artifacts={[]}
      onNewArtifact={() => {}}
      onOpenArtifact={() => {}}
      projectFilter={projectFilter}
      onProjectFilter={onProjectFilter}
      showDone={false}
      onShowDone={() => {}}
      doneCount={0}
      openId={null}
      onOpen={() => {}}
      onNew={() => {}}
      onChanged={() => {}}
    />,
  );
  return onProjectFilter;
}

beforeEach(() => localStorage.clear());

describe("Tasks project filter", () => {
  it("has no native select and opens the shared popover with a check on the current filter", async () => {
    setup("all");
    expect(document.querySelector("select")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Project" }));
    expect(document.querySelector(".sel-pop")).not.toBeNull();
    expect(screen.getByRole("option", { name: "No project" })).toBeTruthy();
    expect(screen.getByRole("option", { name: "All projects" }).getAttribute("aria-selected")).toBe("true");
  });

  it("shows only tasks without a project for No project", () => {
    setup("none");
    expect(screen.queryByText("Title HUL-1")).toBeNull();
    expect(screen.getByText("Title HUL-2")).toBeTruthy();
  });

  it("reports the chosen filter", async () => {
    const onProjectFilter = setup("all");
    await userEvent.click(screen.getByRole("button", { name: "Project" }));
    await userEvent.click(screen.getByRole("option", { name: "No project" }));
    expect(onProjectFilter).toHaveBeenCalledWith("none");
  });
});
