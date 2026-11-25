import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Artifact } from "../api";
import ArtifactsSection from "./ArtifactsSection";

const artifact = (over: Partial<Artifact>): Artifact => ({
  id: "a1",
  projectId: null,
  milestoneId: null,
  issueId: "i1",
  title: "Plan for login",
  kind: "document",
  filename: "",
  mime: "",
  size: 0,
  sha256: "",
  createdAt: "2026-01-01T00:00:00Z",
  type: "plan",
  content: "",
  files: [],
  author: "Ada",
  authorIsAgent: false,
  updatedAt: "2026-01-01T00:00:00Z",
  ...over,
});

describe("ArtifactsSection", () => {
  it("lists the task's artifacts and opens one", async () => {
    const onOpen = vi.fn();
    render(<ArtifactsSection artifacts={[artifact({ files: [{ id: "f", filename: "x", mime: "", size: 1 }] })]} onNew={vi.fn()} onOpen={onOpen} />);
    expect(screen.getByText("Artifacts · 1")).toBeTruthy();
    expect(screen.getByText("Plan")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Plan for login/ }));
    expect(onOpen).toHaveBeenCalledWith("a1");
  });

  it("New asks for the shared dialog", async () => {
    const onNew = vi.fn();
    render(<ArtifactsSection artifacts={[]} onNew={onNew} onOpen={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "New" }));
    expect(onNew).toHaveBeenCalledTimes(1);
  });
});
