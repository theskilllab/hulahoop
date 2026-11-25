import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, describe, expect, it, vi } from "vitest";
import type { Milestone } from "../api";

vi.mock("../api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api")>()),
  gql: vi.fn(),
  uploadArtifact: vi.fn(),
}));

import Milestones from "./Milestones";

// jsdom has <dialog> but not its modal methods.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

const milestone = (over: Partial<Milestone>): Milestone => ({
  id: "m1",
  projectId: null,
  title: "Beta launch",
  body: "",
  status: "open",
  targetDate: null,
  createdAt: "2026-01-01T00:00:00Z",
  ...over,
});

function setup(milestones: Milestone[]) {
  return render(
    <Milestones
      workspaceId="w1"
      projects={[]}
      milestones={milestones}
      issues={[]}
      artifacts={[]}
      onChanged={() => {}}
      onOpenIssue={() => {}}
      onOpenArtifacts={() => {}}
    />,
  );
}

describe("Milestones page", () => {
  it("opens New milestone as a dialog, not an inline panel", async () => {
    const { container } = setup([milestone({})]);
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "New milestone" }));
    expect(screen.getByRole("dialog", { name: "New milestone" })).toBeTruthy();
    expect(container.querySelector(".ms-new")).toBeNull();
    // The header button no longer turns into Cancel.
    expect(screen.getByRole("button", { name: "New milestone" })).toBeTruthy();
  });

  it("shows No due date once on a list row without a due date", () => {
    const { container } = setup([milestone({})]);
    const row = container.querySelector(".msrow") as HTMLElement;
    const meta = row.querySelector(".ms-meta") as HTMLElement;
    expect(meta.textContent?.match(/No due date/g)).toHaveLength(1);
    expect(meta.textContent).not.toMatch(/No date/);
    expect(within(row).getAllByText(/No due date/)).toHaveLength(1);
  });

  it("still shows the date and the countdown when a due date exists", () => {
    const { container } = setup([milestone({ targetDate: "2999-03-05" })]);
    const meta = container.querySelector(".msrow .ms-meta") as HTMLElement;
    expect(meta.textContent).toContain("Mar 5, 2999");
    expect(meta.textContent).toContain("left");
    expect(meta.textContent).not.toMatch(/No due date/);
  });
});
