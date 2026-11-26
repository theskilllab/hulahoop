import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Artifact, IssueSummary, Milestone, Project } from "../api";

const api = vi.hoisted(() => ({ gql: vi.fn(), createArtifact: vi.fn(), deleteArtifact: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api")>()), ...api }));

import Artifacts from "./Artifacts";

beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

const projects = [
  { id: "p1", name: "Balance", prefix: "BAL", color: "#2f7ad8", repos: [] },
  { id: "p2", name: "Other", prefix: "OTH", color: "#d82f7a", repos: [] },
] as Project[];
const milestones = [
  { id: "m1", projectId: "p2", title: "Beta launch", body: "", status: "open", targetDate: null, createdAt: "" },
] as Milestone[];
const issues = [
  { id: "i1", key: "BAL-1", title: "Fix login", projectId: "p1", status: "in_progress" },
] as unknown as IssueSummary[];

const art = (over: Partial<Artifact>): Artifact =>
  ({
    id: "a",
    projectId: null,
    milestoneId: null,
    issueId: null,
    title: "Untitled",
    kind: "file",
    type: "file",
    content: "",
    files: [],
    author: "Rina",
    authorIsAgent: false,
    filename: "",
    mime: "",
    size: 0,
    sha256: "",
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...over,
  }) as Artifact;

const list = [
  art({ id: "a1", title: "Login plan", type: "plan", issueId: "i1", projectId: "p1", author: "Planner", authorIsAgent: true, updatedAt: "2026-03-01T00:00:00Z" }),
  art({ id: "a2", title: "Launch report", type: "report", milestoneId: "m1", projectId: "p2", updatedAt: "2026-02-01T00:00:00Z" }),
  art({
    id: "a3",
    title: "Brand kit",
    type: "file",
    issueId: "i1",
    projectId: "p1",
    files: [{ id: "f1", filename: "logo.svg", mime: "image/svg+xml", size: 10 }],
    updatedAt: "2026-01-15T00:00:00Z",
  }),
] as Artifact[];

function setup(artifacts = list) {
  const onChanged = vi.fn();
  const view = render(
    <Artifacts workspaceId="w1" projects={projects} milestones={milestones} issues={issues} artifacts={artifacts} onChanged={onChanged} />,
  );
  return { onChanged, ...view };
}

const titles = (container: HTMLElement) =>
  Array.from(container.querySelectorAll(".arow:not(.head) .a-title")).map((n) => n.textContent);

beforeEach(() => {
  api.deleteArtifact.mockResolvedValue(true);
  api.gql.mockResolvedValue({ deleteArtifact: true });
});

describe("Artifacts list", () => {
  it("has the title, count and controls in order, with no native select", () => {
    const { container } = setup();
    expect(screen.getByRole("heading", { name: "Artifacts" })).toBeTruthy();
    expect(screen.getByText("3 stored")).toBeTruthy();
    expect(screen.getByPlaceholderText("Search artifacts")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Project/ }).textContent).toContain("All projects");
    expect(screen.getByRole("button", { name: /Type/ }).textContent).toContain("All types");
    expect(screen.getByRole("button", { name: "New artifact" })).toBeTruthy();
    expect(container.querySelector("select")).toBeNull();
    expect(container.querySelector("table")).toBeNull();
  });

  it("shows a header row and lists newest first", () => {
    const { container } = setup();
    const head = container.querySelector(".arow.head") as HTMLElement;
    expect(head.textContent).toMatch(/Artifact.*Belongs to.*Status.*Updated/);
    expect(titles(container)).toEqual(["Login plan", "Launch report", "Brand kit"]);
  });

  it("shows the type chip, author, agent marker, owner and file badge", () => {
    const { container } = setup();
    const [plan, report, kit] = Array.from(container.querySelectorAll(".arow:not(.head)")) as HTMLElement[];
    expect(plan.textContent).toContain("Plan");
    expect(plan.textContent).toContain("Planner · agent");
    expect(plan.textContent).toContain("BAL-1");
    expect(plan.textContent).toContain("Fix login");
    expect(kit.querySelector(".a-title svg, .a-title .count")).toBeTruthy();
    expect(report.textContent).toContain("Beta launch");
    expect(report.textContent).toContain("Other");
  });

  it("filters by search over title, task, milestone, type, author and file names", async () => {
    const { container } = setup();
    const search = screen.getByPlaceholderText("Search artifacts");
    for (const [query, expected] of [
      ["brand", ["Brand kit"]],
      ["BAL-1", ["Login plan", "Brand kit"]],
      ["beta", ["Launch report"]],
      ["report", ["Launch report"]],
      ["planner", ["Login plan"]],
      ["logo.svg", ["Brand kit"]],
    ] as const) {
      await userEvent.clear(search);
      await userEvent.type(search, query);
      expect(titles(container), query).toEqual(expected);
    }
  });

  it("filters by type through the dropdown", async () => {
    const { container } = setup();
    await userEvent.click(screen.getByRole("button", { name: /Type/ }));
    await userEvent.click(screen.getByRole("option", { name: "Reports" }));
    expect(titles(container)).toEqual(["Launch report"]);
  });

  it("filters by project through the dropdown", async () => {
    const { container } = setup();
    await userEvent.click(screen.getByRole("button", { name: /Project/ }));
    await userEvent.click(screen.getByRole("option", { name: /Balance/ }));
    expect(titles(container)).toEqual(["Login plan", "Brand kit"]);
  });

  it("explains an empty shelf and an empty filter", async () => {
    const copy = "Agents save their plans here automatically when they start a task. You can also add your own.";
    const first = setup([]);
    expect(screen.getByText("No artifacts yet")).toBeTruthy();
    expect(screen.getByText(copy)).toBeTruthy();
    first.unmount();
    setup();
    await userEvent.type(screen.getByPlaceholderText("Search artifacts"), "zzzz");
    expect(screen.getByText("No artifacts match these filters")).toBeTruthy();
    expect(screen.getByText(copy)).toBeTruthy();
  });

  it("opens New artifact as a modal dialog, not an inline panel", async () => {
    const { container } = setup();
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "New artifact" }));
    expect(screen.getByRole("dialog", { name: "New artifact" })).toBeTruthy();
    expect(container.querySelector(".panel")).toBeNull();
    expect(screen.queryByText("Add a file")).toBeNull();
  });

  it("right-click offers Open and a red Delete…", () => {
    const { container } = setup();
    fireEvent.contextMenu(container.querySelector(".arow:not(.head)") as HTMLElement);
    const menu = screen.getByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: /Open/ })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: /Delete…/ }).className).toContain("danger");
  });

  it("asks before deleting, and deletes only on the red Delete", async () => {
    const { container, onChanged } = setup();
    fireEvent.contextMenu(container.querySelector(".arow:not(.head)") as HTMLElement);
    await userEvent.click(screen.getByRole("menuitem", { name: /Delete…/ }));
    const alert = screen.getByRole("alert");
    expect(alert.className).toContain("review-bar");
    expect(alert.textContent).toContain("Delete this artifact?");
    expect(alert.textContent).toContain("BAL-1");
    expect(api.deleteArtifact.mock.calls.length + api.gql.mock.calls.length).toBe(0);
    await userEvent.click(within(alert).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(api.deleteArtifact).not.toHaveBeenCalled();

    fireEvent.contextMenu(container.querySelector(".arow:not(.head)") as HTMLElement);
    await userEvent.click(screen.getByRole("menuitem", { name: /Delete…/ }));
    await userEvent.click(within(screen.getByRole("alert")).getByRole("button", { name: "Delete" }));
    const calls = [...api.deleteArtifact.mock.calls, ...api.gql.mock.calls];
    expect(JSON.stringify(calls)).toContain("a1");
    expect(onChanged).toHaveBeenCalled();
  });
});
