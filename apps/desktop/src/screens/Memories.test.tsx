import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  loadMemories: vi.fn(),
  updateMemory: vi.fn(),
  forgetMemory: vi.fn(),
}));

vi.mock("../api", () => api);

import Memories from "./Memories";

const note = {
  id: "m1",
  scope: "workspace",
  scopeRef: "",
  kind: "gotcha",
  title: "envrc is blocked",
  body: "Run commands through direnv.",
  sourceIssueId: "i1",
  sourceIssueKey: "HUL-1",
  author: "Reviewer",
  updatedAt: "2025-01-01T00:00:00Z",
  useCount: 2,
};

beforeEach(() => {
  vi.resetAllMocks();
  api.loadMemories.mockResolvedValue([note]);
});

describe("Memories", () => {
  it("lists notes with their source and opens the task they came from", async () => {
    const onOpenTask = vi.fn();
    render(<Memories workspaceId="w" projects={[]} onOpenTask={onOpenTask} />);
    expect(await screen.findByText("envrc is blocked")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "HUL-1" }));
    expect(onOpenTask).toHaveBeenCalledWith("i1");
  });

  it("filters by scope and by search words", async () => {
    render(<Memories workspaceId="w" projects={[]} onOpenTask={() => {}} />);
    await screen.findByText("envrc is blocked");
    await userEvent.click(screen.getByRole("button", { name: "People" }));
    expect(api.loadMemories).toHaveBeenLastCalledWith("w", { scope: "person", query: "" });
    await userEvent.type(screen.getByLabelText("Search memories"), "x");
    expect(api.loadMemories).toHaveBeenLastCalledWith("w", { scope: "person", query: "x" });
  });

  it("edits a note", async () => {
    api.updateMemory.mockResolvedValue(note);
    render(<Memories workspaceId="w" projects={[]} onOpenTask={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const title = screen.getByLabelText("Title");
    await userEvent.clear(title);
    await userEvent.type(title, "new title");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(api.updateMemory).toHaveBeenCalledWith("m1", {
      kind: "gotcha",
      title: "new title",
      body: "Run commands through direnv.",
    });
  });

  it("forgets a note only after a second click", async () => {
    api.forgetMemory.mockResolvedValue(true);
    render(<Memories workspaceId="w" projects={[]} onOpenTask={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "Forget" }));
    expect(api.forgetMemory).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Forget for good" }));
    expect(api.forgetMemory).toHaveBeenCalledWith("m1");
  });

  it("shows why a save failed", async () => {
    api.updateMemory.mockRejectedValue(new Error("a memory needs a title"));
    render(<Memories workspaceId="w" projects={[]} onOpenTask={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("a memory needs a title")).toBeTruthy();
  });
});
