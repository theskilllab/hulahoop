import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  loadGithubAccount: vi.fn(),
  setGithubToken: vi.fn(),
  clearGithubToken: vi.fn(),
  loadModels: vi.fn(),
  setWorkspaceLumi: vi.fn(),
  updateWorkspace: vi.fn(),
}));

vi.mock("../api", () => api);

import { GithubAccountBlock } from "./Settings";

const account = {
  login: "rez",
  name: "Rez Muh",
  avatarUrl: null,
  tokenHint: "github_pat_…a1B2",
  validatedAt: new Date().toISOString(),
  problems: [] as string[],
};

beforeEach(() => {
  api.loadGithubAccount.mockResolvedValue(null);
});

describe("GithubAccountBlock", () => {
  it("asks for a token when there is none, and explains the permissions", async () => {
    render(<GithubAccountBlock />);
    const input = (await screen.findByLabelText("Personal access token")) as HTMLInputElement;
    expect(input.type).toBe("password");
    expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(/Pull requests \(read and write\)/)).toBeTruthy();
  });

  it("saves a token and shows the account it belongs to", async () => {
    api.setGithubToken.mockResolvedValue(account);
    render(<GithubAccountBlock />);
    await userEvent.type(await screen.findByLabelText("Personal access token"), "github_pat_secret{Enter}");
    expect(api.setGithubToken).toHaveBeenCalledWith("github_pat_secret");
    expect(await screen.findByText("Rez Muh")).toBeTruthy();
    expect(screen.getByText("github_pat_…a1B2")).toBeTruthy();
    expect(screen.queryByLabelText("Personal access token")).toBeNull();
  });

  it("shows why GitHub refused a token", async () => {
    api.setGithubToken.mockRejectedValue(new Error("GitHub refused the token: Bad credentials"));
    render(<GithubAccountBlock />);
    await userEvent.type(await screen.findByLabelText("Personal access token"), "nope");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect((await screen.findByRole("alert")).textContent).toBe("GitHub refused the token: Bad credentials");
    expect(screen.getByLabelText("Personal access token")).toBeTruthy();
  });

  it("lists what the token cannot do, and replaces or removes it", async () => {
    api.loadGithubAccount.mockResolvedValue({ ...account, problems: ["Cannot read checks on northwind/relay-api"] });
    api.clearGithubToken.mockResolvedValue(true);
    render(<GithubAccountBlock />);
    expect(await screen.findByText("Cannot read checks on northwind/relay-api")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Replace" }));
    expect(screen.getByLabelText("Personal access token")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("Personal access token")).toBeNull();

    // Remove asks first; Keep leaves the token alone.
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(api.clearGithubToken).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(screen.getByText("Rez Muh")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove token" }));
    expect(api.clearGithubToken).toHaveBeenCalledTimes(1);
    expect(await screen.findByLabelText("Personal access token")).toBeTruthy();
  });

  it("shows a failed check as an error with Retry, never as no token", async () => {
    api.loadGithubAccount.mockRejectedValueOnce(new Error("server unreachable"));
    api.loadGithubAccount.mockResolvedValueOnce(account);
    render(<GithubAccountBlock />);
    expect((await screen.findByRole("alert")).textContent).toBe("Could not check your GitHub account: server unreachable");
    expect(screen.queryByLabelText("Personal access token")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Rez Muh")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
