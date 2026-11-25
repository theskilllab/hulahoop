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
  it("uses the filled select style and offers No project", () => {
    setup("all");
    const select = screen.getByLabelText("Project");
    expect(select.className).toContain("select");
    expect(screen.getByRole("option", { name: "No project" })).toBeTruthy();
  });

  it("shows only tasks without a project for No project", () => {
    setup("none");
    expect(screen.queryByText("Title HUL-1")).toBeNull();
    expect(screen.getByText("Title HUL-2")).toBeTruthy();
  });

  it("reports the chosen filter", async () => {
    const onProjectFilter = setup("all");
    await userEvent.selectOptions(screen.getByLabelText("Project"), "none");
    expect(onProjectFilter).toHaveBeenCalledWith("none");
  });
});
