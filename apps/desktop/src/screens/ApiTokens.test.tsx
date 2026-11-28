import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  createApiToken: vi.fn(),
  loadApiTokens: vi.fn(),
  revokeApiToken: vi.fn(),
}));

vi.mock("../api", () => api);

import ApiTokens from "./ApiTokens";

beforeEach(() => {
  vi.resetAllMocks();
});

describe("ApiTokens", () => {
  it("creates a token, shows its value once, and revokes it", async () => {
    const listed = { id: "t1", name: "laptop", createdAt: new Date().toISOString() };
    api.loadApiTokens.mockResolvedValueOnce([]).mockResolvedValueOnce([listed]).mockResolvedValue([]);
    api.createApiToken.mockResolvedValue("hat_secret");
    api.revokeApiToken.mockResolvedValue(undefined);
    render(<ApiTokens />);

    await userEvent.type(await screen.findByLabelText("Token name"), "laptop{Enter}");
    expect(api.createApiToken).toHaveBeenCalledWith("laptop");
    expect(await screen.findByText("hat_secret")).toBeTruthy();

    await userEvent.click(await screen.findByRole("button", { name: "Revoke laptop" }));
    expect(api.revokeApiToken).toHaveBeenCalledWith("t1");
    expect(screen.queryByRole("button", { name: "Revoke laptop" })).toBeNull();
  });

  it("shows the server's refusal", async () => {
    api.loadApiTokens.mockResolvedValue([]);
    api.createApiToken.mockRejectedValue(new Error("that token name is too long"));
    render(<ApiTokens />);
    await userEvent.type(await screen.findByLabelText("Token name"), "x{Enter}");
    expect((await screen.findByRole("alert")).textContent).toBe("that token name is too long");
  });
});
