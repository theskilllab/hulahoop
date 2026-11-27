import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { CostReport, CostTotals } from "../api";

const api = vi.hoisted(() => ({
  loadCostReport: vi.fn(),
  saveCostPlan: vi.fn(),
  saveCostSettings: vi.fn(),
}));

vi.mock("../api", () => api);

import Cost from "./Cost";

const zero: CostTotals = {
  runs: 0, succeeded: 0, failed: 0, cancelled: 0, tokensIn: 0, tokensOut: 0, cacheRead: 0,
  cacheWrite: 0, knownCostRuns: 0, equivalentUsd: 0, knownCashRuns: 0, cashUsd: 0,
  activeRuns: 0, activeMinutes: 0, knownTurnRuns: 0, turns: 0, knownToolRuns: 0, tools: 0,
};

const cohort = { ...zero, tasks: 0, failedRuns: 0, elapsedMinutes: 0 };

// `tokensIn` already holds the cached input, so the screen must not add it again.
const report = {
  period: "30d",
  start: "2026-04-01T00:00:00Z",
  end: "2026-04-02T00:00:00Z",
  priorStart: "2026-03-01T00:00:00Z",
  current: zero,
  previous: zero,
  tasks: cohort,
  priorTasks: cohort,
  month: zero,
  planUsage: [{ ...zero, label: "claude-sub", tokensIn: 1_000, tokensOut: 50, cacheRead: 900, cacheWrite: 40 }],
  monthElapsedDays: 1,
  monthTotalDays: 30,
  unsuccessfulTasks: 0,
  daily: [],
  dailyModels: [],
  agents: [],
  models: [],
  failureReasons: [],
  failures: [],
  settings: { apiBudgetUsd: null, successTargetPercent: null },
  plans: [{ connectorId: "claude-sub", label: "Claude Max", monthlyFeeUsd: 100, allowanceTokens: null }],
} as unknown as CostReport;

describe("Cost", () => {
  it("shows plan usage as input plus output without counting cached input twice", async () => {
    api.loadCostReport.mockResolvedValue(report);
    render(<Cost workspaceId="w" agents={[]} onOpen={() => {}} />);
    expect(await screen.findByText(/1,050 tokens used/)).toBeTruthy();
  });
});
