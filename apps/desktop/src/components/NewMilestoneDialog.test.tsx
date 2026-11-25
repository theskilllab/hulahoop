import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Project } from "../api";

const api = vi.hoisted(() => ({ gql: vi.fn(), uploadArtifact: vi.fn() }));

vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api")>()), ...api }));

import NewMilestoneDialog from "./NewMilestoneDialog";

// jsdom has <dialog> but not its modal methods.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

const projects: Project[] = [
  { id: "p1", name: "Balance", prefix: "BAL", color: "#2f7ad8", repos: [] },
  { id: "p2", name: "Loose", prefix: null, color: "#2f7ad8", repos: [] },
];

function setup() {
  const onCreated = vi.fn();
  const onClose = vi.fn();
  const view = render(<NewMilestoneDialog workspaceId="w1" projects={projects} onCreated={onCreated} onClose={onClose} />);
  return { onCreated, onClose, view };
}

const file = (name: string) => new File(["hello"], name, { type: "text/plain" });

beforeEach(() => {
  api.gql.mockResolvedValue({ createMilestone: { id: "m-new" } });
  api.uploadArtifact.mockResolvedValue({ id: "a1" });
});

describe("NewMilestoneDialog", () => {
  it("is a sheet like New task, titled New milestone", () => {
    setup();
    const dialog = screen.getByRole("dialog", { name: "New milestone" });
    expect(dialog.classList.contains("settings-dialog")).toBe(true);
    expect(dialog.classList.contains("new-milestone-dialog")).toBe(true);
  });

  it("uses the prototype's field labels and copy", () => {
    setup();
    expect(screen.getByLabelText(/^Name/)).toBeTruthy();
    expect(screen.getByLabelText(/^Due date \(optional\)/)).toBeTruthy();
    expect(screen.getByLabelText("Project")).toBeTruthy();
    expect(screen.getByLabelText(/^Description \(optional\)/)).toBeTruthy();
    // The old wording is gone.
    expect(screen.queryByText(/target date/i)).toBeNull();
    expect(screen.queryByText(/what it is for/i)).toBeNull();
    expect(screen.queryByText(/^title$/i)).toBeNull();
  });

  it("offers No project first, then each project with its prefix", () => {
    setup();
    const select = screen.getByLabelText("Project") as HTMLSelectElement;
    const options = Array.from(select.options).map((option) => [option.value, option.textContent]);
    expect(options).toEqual([
      ["", "No project"],
      ["p1", "BAL · Balance"],
      ["p2", "Loose"],
    ]);
    expect(select.value).toBe("");
  });

  it("styles its labels and the drop area like the prototype", () => {
    const { view } = setup();
    expect(screen.getByText(/^Name/).closest("label")?.classList.contains("field-label")).toBe(true);
    expect(view.baseElement.querySelector(".new-milestone-text textarea")).toBeTruthy();
  });

  it("asks for a name of at least three characters, with the prototype's message", async () => {
    setup();
    await userEvent.type(screen.getByLabelText(/^Name/), "ab");
    await userEvent.click(screen.getByRole("button", { name: "Create milestone" }));
    expect(within(screen.getByRole("alert")).getByText("Name the milestone so people know what it is working toward.")).toBeTruthy();
    expect(api.gql).not.toHaveBeenCalled();
    expect(screen.getByLabelText(/^Name/).getAttribute("aria-invalid")).toBe("true");
  });

  it("creates the milestone with name, due date, project and description", async () => {
    const { onCreated, onClose } = setup();
    await userEvent.type(screen.getByLabelText(/^Name/), "  GitHub identity ");
    fireEvent.change(screen.getByLabelText(/^Due date/), { target: { value: "2026-03-05" } });
    await userEvent.selectOptions(screen.getByLabelText("Project"), "p1");
    await userEvent.type(screen.getByLabelText(/^Description/), "Sign in with GitHub");
    await userEvent.click(screen.getByRole("button", { name: "Create milestone" }));

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith({ id: "m-new", title: "GitHub identity" }));
    const [query, variables] = api.gql.mock.calls[0];
    expect(query).toContain("createMilestone");
    expect(variables).toEqual({
      workspaceId: "w1",
      projectId: "p1",
      title: "GitHub identity",
      body: "Sign in with GitHub",
      targetDate: "2026-03-05",
    });
    expect(api.uploadArtifact).not.toHaveBeenCalled();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("sends no project and no date when they are left empty", async () => {
    const { onCreated } = setup();
    await userEvent.type(screen.getByLabelText(/^Name/), "Beta launch");
    await userEvent.click(screen.getByRole("button", { name: "Create milestone" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(api.gql.mock.calls[0][1]).toMatchObject({ projectId: null, targetDate: null, title: "Beta launch" });
  });

  it("highlights the description area while files are dragged over it", () => {
    const { view } = setup();
    const area = view.baseElement.querySelector(".new-milestone-text") as HTMLElement;
    fireEvent.dragOver(area, { dataTransfer: { types: ["Files"], dropEffect: "none" } });
    expect(area.classList.contains("over")).toBe(true);
    fireEvent.dragLeave(area, { dataTransfer: { types: ["Files"] } });
    expect(area.classList.contains("over")).toBe(false);
  });

  it("attaches files dropped on the description as artifacts of the new milestone", async () => {
    const { view, onCreated } = setup();
    const area = view.baseElement.querySelector(".new-milestone-text") as HTMLElement;
    const plan = file("release-plan.txt");
    fireEvent.drop(area, { dataTransfer: { types: ["Files"], files: [plan] } });
    expect(screen.getByText("release-plan.txt")).toBeTruthy();

    await userEvent.type(screen.getByLabelText(/^Name/), "Beta launch");
    await userEvent.selectOptions(screen.getByLabelText("Project"), "p1");
    await userEvent.click(screen.getByRole("button", { name: "Create milestone" }));

    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(api.uploadArtifact).toHaveBeenCalledTimes(1);
    expect(api.uploadArtifact.mock.calls[0][0]).toMatchObject({
      workspaceId: "w1",
      projectId: "p1",
      milestoneId: "m-new",
      issueId: null,
      title: "release-plan.txt",
      file: plan,
    });
  });

  it("attaches files chosen with the picker and lets one be removed", async () => {
    const { view } = setup();
    expect(screen.getByRole("button", { name: "or choose files to attach" })).toBeTruthy();
    const input = view.baseElement.querySelector('input[type="file"]') as HTMLInputElement;
    expect(input.multiple).toBe(true);
    await userEvent.upload(input, [file("a.txt"), file("b.txt")]);
    expect(screen.getByText("a.txt")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Remove a.txt" }));
    expect(screen.queryByText("a.txt")).toBeNull();
    expect(screen.getByText("b.txt")).toBeTruthy();
  });

  it("refuses files dropped outside the description", () => {
    setup();
    const dialog = screen.getByRole("dialog", { name: "New milestone" });
    const dataTransfer = { types: ["Files"], dropEffect: "copy" };
    fireEvent.dragOver(dialog, { dataTransfer });
    expect(dataTransfer.dropEffect).toBe("none");
  });

  it("closes without asking when nothing was typed", async () => {
    const confirm = vi.spyOn(window, "confirm");
    const { onClose } = setup();
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(confirm).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("asks before discarding typed text, and stays open when declined", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    const { onClose } = setup();
    await userEvent.type(screen.getByLabelText(/^Name/), "Beta");
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(confirm).toHaveBeenCalledWith("Discard this milestone? What you typed and the files you added will be lost.");
    expect(onClose).not.toHaveBeenCalled();
    confirm.mockReturnValue(true);
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("closes on Escape through the same discard check", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const { onClose } = setup();
    fireEvent.change(screen.getByLabelText(/^Description/), { target: { value: "notes" } });
    fireEvent(screen.getByRole("dialog", { name: "New milestone" }), new Event("cancel", { cancelable: true }));
    expect(confirm).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("shows the server's error and keeps the sheet open when creating fails", async () => {
    api.gql.mockRejectedValue(new Error("boom"));
    const { onClose, onCreated } = setup();
    await userEvent.type(screen.getByLabelText(/^Name/), "Beta launch");
    await userEvent.click(screen.getByRole("button", { name: "Create milestone" }));
    expect((await screen.findByRole("alert")).textContent).toContain("boom");
    expect(onClose).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });
});
