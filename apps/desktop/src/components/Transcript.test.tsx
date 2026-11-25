import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ loadTranscript: vi.fn() }));
vi.mock("../api", () => api);

import Transcript from "./Transcript";

beforeEach(() => {
  api.loadTranscript.mockResolvedValue({ entries: [], lastSeq: 0, live: false });
});

describe("Transcript header", () => {
  it("says which credential the run started with", async () => {
    render(
      <Transcript runId="r1" runStatus="succeeded" agentName="Builder" issueKey="HUL-12" credentialSource="Codex subscription" onBack={vi.fn()} />,
    );
    await screen.findByText("This run left no transcript.");
    expect((document.querySelector(".tx-bar") as HTMLElement).textContent).toContain("Credential: Codex subscription");
  });

  it("leaves it out when the run did not record one", async () => {
    render(<Transcript runId="r1" runStatus="succeeded" agentName="Builder" issueKey="HUL-12" onBack={vi.fn()} />);
    await screen.findByText("This run left no transcript.");
    expect(screen.queryByText(/Credential:/)).toBeNull();
  });
});
