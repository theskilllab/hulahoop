import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { IssueSummary, RunnerCapacity } from "../api";

const { room, loadRunnerCapacity } = vi.hoisted(() => {
  const room = {
    running: 3,
    waiting: 1,
    freeMemoryBytes: 4_200_000_000,
    freeDiskBytes: 0,
    freeCpuCores: 0,
    maxRuns: 4,
    reportedAt: "2026-01-01T00:00:00Z",
  };
  return { room, loadRunnerCapacity: vi.fn(async (): Promise<typeof room | null> => room) };
});
vi.mock("../api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api")>()),
  loadRunnerCapacity,
}));

import { publishRunnerCapacity } from "../runnerCapacity";
import { capacityLine } from "../screens/Tasks";
import { TaskWait } from "./TaskCard";

afterEach(() => {
  cleanup();
  loadRunnerCapacity.mockImplementation(async () => room);
});

const capacity: RunnerCapacity = room;

const issue = (currentRun: IssueSummary["currentRun"]): IssueSummary =>
  ({ id: "i1", key: "KEP-1", title: "Task", status: "in_progress", currentRun }) as IssueSummary;

describe("TaskWait", () => {
  it("shows why the task's queued run waits", () => {
    const reason = "Waiting for CPU: needs ~1.0 cores, 0.4 free";
    publishRunnerCapacity(capacity);
    render(<TaskWait issue={issue({ id: "r1", agentId: "a1", status: "queued", waitReason: reason })} />);
    expect(screen.getByText(reason)).toBeTruthy();
  });

  it("hides a reason once no runner keeps it fresh", async () => {
    const reason = "Waiting for CPU: needs ~1.0 cores, 0.4 free";
    loadRunnerCapacity.mockImplementation(async () => null);
    publishRunnerCapacity(capacity);
    render(<TaskWait issue={issue({ id: "r1", agentId: "a1", status: "queued", waitReason: reason })} />);
    expect(screen.getByText(reason)).toBeTruthy();
    await act(async () => {
      publishRunnerCapacity(null);
    });
    expect(screen.queryByText(reason)).toBeNull();
  });

  it("says nothing for a run that is going", () => {
    const { container } = render(
      <TaskWait issue={issue({ id: "r1", agentId: "a1", status: "running", waitReason: "Waiting for disk" })} />,
    );
    expect(container.textContent).toBe("");
  });
});

describe("capacityLine", () => {
  it("counts runs and the memory left", () => {
    expect(capacityLine(capacity)).toBe("3 running · 1 waiting · 4.2 GB room");
  });
});
