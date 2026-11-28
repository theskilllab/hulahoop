import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  probe: vi.fn(),
  saveRemote: vi.fn(),
  signIn: vi.fn(),
  signUp: vi.fn(),
  useLocalServer: vi.fn(),
}));

vi.mock("../api", () => api);

import Connect from "./Connect";

beforeEach(() => {
  vi.resetAllMocks();
});

describe("Connect", () => {
  it("signs in with an email and a password", async () => {
    api.signIn.mockResolvedValue(undefined);
    const onConnected = vi.fn();
    render(<Connect onConnected={onConnected} />);

    await userEvent.clear(screen.getByLabelText("server"));
    await userEvent.type(screen.getByLabelText("server"), "https://hula.example");
    await userEvent.type(screen.getByLabelText("email"), "ada@example.com");
    await userEvent.type(screen.getByLabelText("password"), "correct horse{Enter}");

    expect(api.signIn).toHaveBeenCalledWith("https://hula.example", "ada@example.com", "correct horse");
    expect(onConnected).toHaveBeenCalled();
  });

  it("shows a refused sign-in and stays on the form", async () => {
    api.signIn.mockRejectedValue(new Error("wrong email or password"));
    const onConnected = vi.fn();
    render(<Connect onConnected={onConnected} />);
    await userEvent.type(screen.getByLabelText("email"), "ada@example.com");
    await userEvent.type(screen.getByLabelText("password"), "nope{Enter}");
    expect(await screen.findByText("wrong email or password")).toBeTruthy();
    expect(onConnected).not.toHaveBeenCalled();
  });

  it("signs up with an invite code", async () => {
    api.signUp.mockResolvedValue(undefined);
    render(<Connect onConnected={vi.fn()} />);
    await userEvent.click(screen.getByRole("tab", { name: "sign up" }));
    await userEvent.type(screen.getByLabelText("name"), "Grace");
    await userEvent.type(screen.getByLabelText("email"), "grace@example.com");
    await userEvent.type(screen.getByLabelText("password"), "another long password");
    await userEvent.type(screen.getByLabelText("invite code"), "hinv_abc{Enter}");
    expect(api.signUp).toHaveBeenCalledWith("http://", {
      name: "Grace",
      email: "grace@example.com",
      password: "another long password",
      invite: "hinv_abc",
    });
  });

  it("still connects with a server token", async () => {
    api.probe.mockResolvedValue(undefined);
    render(<Connect onConnected={vi.fn()} />);
    await userEvent.click(screen.getByRole("tab", { name: "server token" }));
    await userEvent.type(screen.getByLabelText("token"), "tok{Enter}");
    expect(api.probe).toHaveBeenCalledWith("http://", "tok");
    expect(api.saveRemote).toHaveBeenCalledWith("http://", "tok");
  });
});
