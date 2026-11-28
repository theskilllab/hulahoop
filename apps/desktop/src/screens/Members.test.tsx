import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  createInvite: vi.fn(),
  loadMe: vi.fn(),
  loadMembers: vi.fn(),
  loadRegistration: vi.fn(),
  removeMember: vi.fn(),
  setMemberRole: vi.fn(),
  setRegistration: vi.fn(),
}));

vi.mock("../api", () => api);

import Members from "./Members";

const ada = { userId: "u1", name: "Ada", email: "ada@example.com", role: "owner" };
const grace = { userId: "u2", name: "Grace", email: "grace@example.com", role: "member" };

beforeEach(() => {
  vi.resetAllMocks();
  api.loadMembers.mockResolvedValue([ada, grace]);
  api.loadRegistration.mockResolvedValue("closed");
});

describe("Members", () => {
  it("lets an owner change registration, roles and invite someone", async () => {
    api.loadMe.mockResolvedValue({ ...ada, id: "u1" });
    api.setRegistration.mockResolvedValue(undefined);
    api.setMemberRole.mockResolvedValue(undefined);
    api.createInvite.mockResolvedValue("hinv_secret");
    render(<Members workspaceId="w1" />);

    await userEvent.click(await screen.findByRole("radio", { name: /Open/ }));
    expect(api.setRegistration).toHaveBeenCalledWith("open");

    await userEvent.selectOptions(await screen.findByLabelText("Role of Grace"), "owner");
    expect(api.setMemberRole).toHaveBeenCalledWith("w1", "u2", "owner");

    await userEvent.click(screen.getByRole("button", { name: "Create invite" }));
    expect(api.createInvite).toHaveBeenCalledWith("member");
    expect(await screen.findByText("hinv_secret")).toBeTruthy();
  });

  it("shows a member the list read-only, with registration locked", async () => {
    api.loadMe.mockResolvedValue({ ...grace, id: "u2" });
    render(<Members workspaceId="w1" />);

    expect(await screen.findByText("Only an owner can change this.")).toBeTruthy();
    expect((screen.getByRole("radio", { name: /Open/ }) as HTMLInputElement).disabled).toBe(true);
    expect(screen.queryByLabelText("Role of Grace")).toBeNull();
    expect(screen.queryByRole("button", { name: "Create invite" })).toBeNull();
  });

  it("shows why the server refused a change", async () => {
    api.loadMe.mockResolvedValue({ ...ada, id: "u1" });
    api.removeMember.mockRejectedValue(new Error("a workspace needs at least one owner"));
    render(<Members workspaceId="w1" />);
    await userEvent.click(await screen.findByRole("button", { name: "Remove Ada" }));
    expect((await screen.findByRole("alert")).textContent).toBe("a workspace needs at least one owner");
  });
});
