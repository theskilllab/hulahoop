import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { IssueSummary, Milestone, Project } from "../api";

const api = vi.hoisted(() => ({ gql: vi.fn(), createArtifact: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<typeof import("../api")>()), ...api }));

import { PartialArtifactError } from "../api";
import NewArtifactDialog from "./NewArtifactDialog";

beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

const projects = [{ id: "p1", name: "Balance", prefix: "BAL", color: "#2f7ad8", repos: [] }] as Project[];
const milestones = [
  { id: "m1", projectId: "p1", title: "Beta launch", body: "", status: "open", targetDate: null, createdAt: "" },
] as Milestone[];
const issues = [{ id: "i1", key: "BAL-1", title: "Fix login", projectId: "p1" }] as unknown as IssueSummary[];

function setup(preset?: { issueId?: string; milestoneId?: string }) {
  const onCreated = vi.fn();
  const onClose = vi.fn();
  const onToast = vi.fn();
  render(
    <NewArtifactDialog
      workspaceId="w1"
      projects={projects}
      milestones={milestones}
      issues={issues}
      preset={preset}
      onCreated={onCreated}
      onClose={onClose}
      onToast={onToast}
    />,
  );
  return { onCreated, onClose, onToast };
}

const file = (name: string, bytes = 5) => new File([new Uint8Array(bytes)], name, { type: "text/plain" });
const drop = (files: File[]) =>
  fireEvent.drop(screen.getByRole("dialog", { name: "New artifact" }), {
    dataTransfer: { files, types: ["Files"] },
  });

beforeEach(() => {
  api.createArtifact.mockResolvedValue({ id: "a1", title: "x" });
});

describe("NewArtifactDialog", () => {
  it("lays out the prototype fields and autofocuses the title", () => {
    setup();
    const dialog = screen.getByRole("dialog", { name: "New artifact" });
    expect(screen.getByLabelText(/^Title/)).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByLabelText(/^Title/));
    expect(screen.getByRole("button", { name: /Type/ }).textContent).toContain("Plan");
    expect(screen.getByRole("button", { name: /Belongs to/ })).toBeTruthy();
    const content = screen.getByLabelText(/^Content/) as HTMLTextAreaElement;
    expect(content.rows).toBe(5);
    expect(dialog.querySelector("select")).toBeNull();
    expect(screen.queryByRole("button", { name: /attach/i })).toBeNull();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Create artifact" })).toBeTruthy();
  });

  it("offers tasks and milestones in one Belongs to list", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: /Belongs to/ }));
    const pop = document.querySelector(".sel-pop") as HTMLElement;
    expect(pop.closest("dialog")).toBeTruthy();
    expect(within(pop).getByText("Tasks")).toBeTruthy();
    expect(within(pop).getByText("Milestones")).toBeTruthy();
    expect(within(pop).getByRole("option", { name: "BAL-1 · Fix login" })).toBeTruthy();
    expect(within(pop).getByRole("option", { name: /Beta launch/ })).toBeTruthy();
  });

  it("presets Belongs to", () => {
    setup({ issueId: "i1" });
    expect(screen.getByRole("button", { name: /Belongs to/ }).textContent).toContain("BAL-1 · Fix login");
  });

  it("requires a title or a file, with aria-invalid and the prototype message", async () => {
    const { onCreated } = setup({ issueId: "i1" });
    await userEvent.click(screen.getByRole("button", { name: "Create artifact" }));
    expect(screen.getByText("Give the artifact a title, or attach a file.")).toBeTruthy();
    expect(screen.getByLabelText(/^Title/).getAttribute("aria-invalid")).toBe("true");
    expect(api.createArtifact).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });

  it("submits with Enter in the title, using the plan template when content is empty", async () => {
    const { onCreated, onToast } = setup({ issueId: "i1" });
    await userEvent.type(screen.getByLabelText(/^Title/), "Login plan{Enter}");
    await waitFor(() => expect(api.createArtifact).toHaveBeenCalled());
    const input = api.createArtifact.mock.calls[0][0];
    expect(input).toMatchObject({ workspaceId: "w1", title: "Login plan", type: "plan", issueId: "i1", milestoneId: null });
    expect(input.content).toBe("## Goal\n\n## Steps\n\n## Risks\n");
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
    expect(onToast).toHaveBeenCalledWith("Saved to Artifacts, BAL-1", expect.anything());
  });

  it("names the milestone in the toast", async () => {
    const { onToast } = setup({ milestoneId: "m1" });
    await userEvent.type(screen.getByLabelText(/^Title/), "Notes{Enter}");
    await waitFor(() => expect(onToast).toHaveBeenCalled());
    expect(onToast.mock.calls[0][0]).toBe("Saved to Artifacts, Beta launch");
  });

  it("a dropped file switches an empty Plan to File and becomes the title", async () => {
    setup({ issueId: "i1" });
    drop([file("spec-v2.pdf")]);
    expect(screen.getByText("spec-v2.pdf")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Type/ }).textContent).toContain("File");
    await userEvent.click(screen.getByRole("button", { name: "Create artifact" }));
    await waitFor(() => expect(api.createArtifact).toHaveBeenCalled());
    const input = api.createArtifact.mock.calls[0][0];
    expect(input.title).toBe("spec-v2");
    expect(input.type).toBe("file");
    expect(input.files).toHaveLength(1);
  });

  it("keeps Type when the plan already has content, and lets a file be removed", async () => {
    setup({ issueId: "i1" });
    await userEvent.type(screen.getByLabelText(/^Content/), "hello");
    drop([file("a.txt")]);
    expect(screen.getByRole("button", { name: /Type/ }).textContent).toContain("Plan");
    await userEvent.click(screen.getByRole("button", { name: /remove a\.txt/i }));
    expect(screen.queryByText("a.txt")).toBeNull();
  });

  it("rejects a file over 25 MB with a toast", () => {
    const { onToast } = setup({ issueId: "i1" });
    drop([file("huge.bin", 25 * 1024 * 1024 + 1)]);
    expect(onToast).toHaveBeenCalledWith(expect.stringContaining("huge.bin"), true);
    expect(screen.queryByText("huge.bin")).toBeNull();
  });

  it("highlights the dialog while a file is over it", () => {
    setup();
    const dialog = screen.getByRole("dialog", { name: "New artifact" });
    fireEvent.dragOver(dialog, { dataTransfer: { types: ["Files"] } });
    expect(dialog.classList.contains("drop")).toBe(true);
    fireEvent.dragLeave(dialog, { dataTransfer: { types: ["Files"] } });
    expect(dialog.classList.contains("drop")).toBe(false);
  });

  it("opens the artifact instead of creating a second one when a file upload fails", async () => {
    const created = { id: "a1", title: "x" };
    api.createArtifact.mockRejectedValue(new PartialArtifactError(created as never, new Error("disk full")));
    const { onCreated, onClose, onToast } = setup({ issueId: "i1" });
    await userEvent.type(screen.getByLabelText(/^Title/), "Spec");
    await userEvent.click(screen.getByRole("button", { name: "Create artifact" }));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created));
    expect(onToast).toHaveBeenCalledWith(expect.stringContaining("disk full"), true);
    expect(onClose).toHaveBeenCalled();
  });

  it("Cancel and Escape close without saving", async () => {
    const { onClose, onCreated } = setup();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent(screen.getByRole("dialog", { name: "New artifact" }), new Event("cancel", { cancelable: true }));
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(api.createArtifact).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
  });
});
