import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Agent, Project } from "../api";

const api = vi.hoisted(() => ({ createIssue: vi.fn(), uploadArtifact: vi.fn() }));

vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api")>()), ...api }));

import NewTaskDialog from "./NewTaskDialog";

// jsdom has <dialog> but not its modal methods.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
  // ProseMirror measures the caret when text is typed; jsdom has no layout.
  const noRects = () => [] as unknown as DOMRectList;
  Range.prototype.getClientRects = noRects;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
  Element.prototype.getClientRects = noRects;
  document.elementFromPoint = () => null;
});

const projects: Project[] = [{ id: "p1", name: "Balance", prefix: "BAL", color: "#2f7ad8", repos: [] }];
const agents = ["Ada", "Bea", "Cyd", "Dov", "Eli"].map((name) => ({ id: `a-${name}`, name }) as unknown as Agent);

function setup() {
  const onCreated = vi.fn();
  const onClose = vi.fn();
  render(
    <NewTaskDialog workspaceId="w1" projects={projects} agents={agents} milestones={[]} defaultProjectId="p1" onCreated={onCreated} onClose={onClose} />,
  );
  return { onCreated, onClose };
}

beforeEach(() => {
  api.createIssue.mockReset();
  api.createIssue.mockResolvedValue({ id: "i1", key: "BAL-1", projectId: "p1" });
});

describe("NewTaskDialog dropdowns", () => {
  it("has no native select", () => {
    setup();
    expect(document.querySelector("select")).toBeNull();
    for (const name of ["Project", "Priority", "Milestone", "Assign to"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
  });

  it("opens the popover with a check on the current priority", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: "Priority" }));
    expect(document.querySelector(".sel-pop")).not.toBeNull();
    const current = screen.getAllByRole("option").filter((option) => option.getAttribute("aria-selected") === "true");
    expect(current).toHaveLength(1);
    expect(current[0].textContent).toMatch(/medium/i);
  });

  it("filters Assign to by typing, picks with the keyboard and creates with that assignee", async () => {
    const { onCreated } = setup();
    await userEvent.click(screen.getByRole("button", { name: "Assign to" }));
    await userEvent.type(screen.getByRole("textbox", { name: "Search Assign to" }), "cy");
    expect(screen.getAllByRole("option")).toHaveLength(1);
    await userEvent.keyboard("{Enter}");
    expect(document.querySelector(".sel-pop")).toBeNull();
    expect(screen.getByRole("button", { name: "Assign to" }).textContent).toContain("Cyd");

    await userEvent.type(document.querySelector("[contenteditable]") as HTMLElement, "Do the thing");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await vi.waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(api.createIssue).toHaveBeenCalledWith(expect.objectContaining({ assigneeType: "agent", assigneeId: "a-Cyd" }));
  });

  it("closes only the popover on Escape and returns focus to the control", async () => {
    const { onClose } = setup();
    // The editor takes focus when it mounts; let it, so it cannot take it back later.
    await vi.waitFor(() => expect(document.activeElement).toBe(document.querySelector("[contenteditable]")));
    const trigger = screen.getByRole("button", { name: "Assign to" });
    await userEvent.click(trigger);
    await userEvent.keyboard("{Escape}");
    expect(document.querySelector(".sel-pop")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });
});
