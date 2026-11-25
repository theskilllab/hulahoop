import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectorRow, SignIn } from "../api";

const api = vi.hoisted(() => ({
  gql: vi.fn(),
  loadConnections: vi.fn(),
  setConnectorPlan: vi.fn(),
  startSignIn: vi.fn(),
  loadSignIn: vi.fn(),
  submitSignInCode: vi.fn(),
  cancelSignIn: vi.fn(),
}));

vi.mock("../api", () => api);

import { Connectors, SignInDialog } from "./Connectors";

// jsdom has <dialog> but not its modal methods.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

const row = (over: Partial<ConnectorRow>): ConnectorRow => ({
  id: "claude-sub",
  provider: "claude_code",
  kind: "subscription",
  group: "Agent access",
  name: "Claude subscription",
  icon: "spark",
  desc: "Sign in with your Claude plan.",
  enables: "Claude models in Claude Code and Pi",
  connected: false,
  label: null,
  appId: null,
  connectedAt: null,
  harnesses: ["claude_code", "pi"],
  plans: [],
  plan: null,
  ...over,
});

const entries = (): ConnectorRow[] => [
  row({}),
  row({ id: "claude-api", kind: "api_key", name: "Claude API key", enables: "Claude models in Claude Code and Pi" }),
  row({ id: "codex-sub", provider: "openai_codex", name: "Codex subscription", enables: "OpenAI Codex models in Pi", harnesses: ["pi"] }),
  row({ id: "opencode-go", provider: "opencode_go", kind: "api_key", name: "OpenCode Go key", enables: "OpenCode Go models in Pi", harnesses: ["pi"] }),
  row({ id: "pi", provider: "pi", name: "Pi login", enables: "Pi", harnesses: ["pi"] }),
];

const signIn = (over: Partial<SignIn> = {}): SignIn => ({
  id: "s1",
  connectorId: "claude-sub",
  state: "waiting",
  url: "https://claude.ai/oauth/authorize?code=true",
  userCode: null,
  needsCode: false,
  error: null,
  ...over,
});

beforeEach(() => {
  api.loadConnections.mockResolvedValue(entries());
  api.cancelSignIn.mockResolvedValue(true);
  api.loadSignIn.mockImplementation(() => new Promise(() => {}));
});

describe("Connectors", () => {
  it("lists the five agent access entries with what each enables, and how each connects", async () => {
    render(<Connectors />);
    const claude = (await screen.findByText("Claude subscription")).closest(".cn-row") as HTMLElement;
    expect(within(claude).getByText("Claude models in Claude Code and Pi")).toBeTruthy();
    expect(within(claude).getByRole("button", { name: "Sign in to Claude" })).toBeTruthy();
    expect(within(claude).getByRole("button", { name: "Use a setup token" })).toBeTruthy();

    const codex = screen.getByText("Codex subscription").closest(".cn-row") as HTMLElement;
    expect(within(codex).getByText("OpenAI Codex models in Pi")).toBeTruthy();
    expect(within(codex).getByRole("button", { name: "Sign in to Codex" })).toBeTruthy();
    expect(within(codex).queryByRole("button", { name: "Use a setup token" })).toBeNull();

    const go = screen.getByText("OpenCode Go key").closest(".cn-row") as HTMLElement;
    await userEvent.click(within(go).getByRole("button", { name: "Connect" }));
    const dialog = screen.getByRole("dialog", { name: "Connect OpenCode Go key" });
    await userEvent.type(within(dialog).getByLabelText(/^API key/), "go-key");
    api.gql.mockResolvedValue({});
    await userEvent.click(within(dialog).getByRole("button", { name: "Connect" }));
    expect(api.gql).toHaveBeenCalledWith(expect.stringContaining("setAgentKey"), { id: "opencode-go", key: "go-key" });
  });

  it("offers plan names for a connected entry and saves the choice", async () => {
    const plans = [
      { id: "pro", name: "Pro" },
      { id: "max", name: "Max" },
    ];
    api.loadConnections.mockResolvedValue([row({ connected: true, label: "ada@example.com", plans })]);
    api.setConnectorPlan.mockResolvedValue(row({ connected: true, label: "ada@example.com", plans, plan: "max" }));
    render(<Connectors />);
    const select = (await screen.findByLabelText("Plan")) as HTMLSelectElement;
    const options = within(select).getAllByRole("option").map((option) => option.textContent);
    expect(options).toEqual(["Choose your plan", "Pro", "Max"]);
    expect(options.join(" ")).not.toMatch(/\$|\d/);

    await userEvent.selectOptions(select, "max");
    expect(api.setConnectorPlan).toHaveBeenCalledWith("claude-sub", "max");
    await waitFor(() => expect(select.value).toBe("max"));

    api.setConnectorPlan.mockResolvedValue(row({ connected: true, plans, plan: null }));
    await userEvent.selectOptions(select, "");
    expect(api.setConnectorPlan).toHaveBeenLastCalledWith("claude-sub", null);
  });

  it("has no plan picker for entries without plans or that are not connected", async () => {
    api.loadConnections.mockResolvedValue([
      row({ id: "claude-api", connected: true, plans: [] }),
      row({ id: "codex-sub", connected: false, plans: [{ id: "plus", name: "Plus" }] }),
    ]);
    render(<Connectors />);
    await screen.findAllByText("Claude subscription");
    expect(screen.queryByLabelText("Plan")).toBeNull();
  });

  it("says which Pi models are ready when only some providers are connected", async () => {
    api.loadConnections.mockResolvedValue(
      entries().map((entry) => ({ ...entry, connected: entry.id === "claude-sub" || entry.id === "opencode-go" })),
    );
    render(<Connectors />);
    expect(await screen.findByText(/Pi ready for Claude and OpenCode Go models/)).toBeTruthy();
    expect(screen.getByText(/Claude Code ready/)).toBeTruthy();
  });

  it("calls Pi ready with the Pi login or every provider, and asks for a connector with none", async () => {
    api.loadConnections.mockResolvedValue(entries().map((entry) => ({ ...entry, connected: entry.id === "pi" })));
    const { unmount } = render(<Connectors />);
    expect(await screen.findByText(/^\s*Pi ready$/)).toBeTruthy();
    expect(screen.getByText("Claude Code needs a connector")).toBeTruthy();
    unmount();

    api.loadConnections.mockResolvedValue(
      entries().map((entry) => ({ ...entry, connected: ["claude-api", "codex-sub", "opencode-go"].includes(entry.id) })),
    );
    const again = render(<Connectors />);
    expect(await screen.findByText(/^\s*Pi ready$/)).toBeTruthy();
    again.unmount();

    api.loadConnections.mockResolvedValue(entries());
    render(<Connectors />);
    expect(await screen.findByText("Pi needs a connector")).toBeTruthy();
  });

  it("signs in, then closes, refreshes and says so", async () => {
    api.startSignIn.mockResolvedValue(signIn({ connectorId: "codex-sub", url: "https://auth.openai.com/codex/device", userCode: "1MRK-O9PDA" }));
    api.loadSignIn.mockResolvedValue(signIn({ connectorId: "codex-sub", state: "done" }));
    render(<Connectors />);
    await userEvent.click(await screen.findByRole("button", { name: "Sign in to Codex" }));
    expect(api.startSignIn).toHaveBeenCalledWith("codex-sub");
    expect(await screen.findByText("Signed in to Codex", {}, { timeout: 3000 })).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.loadConnections).toHaveBeenCalledTimes(2);
  });
});

describe("SignInDialog", () => {
  const claude = { id: "claude-sub", name: "Claude subscription" };

  it("shows the sign-in page as a link with a copy button", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    api.startSignIn.mockResolvedValue(signIn());
    render(<SignInDialog row={claude} onClose={vi.fn()} onDone={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Sign in to Claude" });
    const link = (await within(dialog).findByRole("link")) as HTMLAnchorElement;
    expect(link.href).toBe("https://claude.ai/oauth/authorize?code=true");
    expect(link.target).toBe("_blank");
    expect(link.rel).toBe("noreferrer");
    await userEvent.click(within(dialog).getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith("https://claude.ai/oauth/authorize?code=true");
    expect(await within(dialog).findByRole("button", { name: "Copied" })).toBeTruthy();
  });

  it("announces a copy politely and reads Copy again after about two seconds", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    api.startSignIn.mockResolvedValue(signIn({ userCode: "1MRK-O9PDA" }));
    render(<SignInDialog row={claude} onClose={vi.fn()} onDone={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "Sign in to Claude" });
    const live = dialog.querySelector("[aria-live]") as HTMLElement;
    expect(live.getAttribute("aria-live")).toBe("polite");
    expect(live.textContent).toBe("");

    await userEvent.click(await within(dialog).findByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("1MRK-O9PDA");
    await waitFor(() => expect(live.textContent).toBe("Code copied"));
    expect(within(dialog).getByRole("button", { name: "Copied" })).toBeTruthy();

    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Copy code" })).toBeTruthy(), {
      timeout: 3000,
    });
    expect(live.textContent).toBe("");
  });

  it("shows a device code large, and polls until the login ends", async () => {
    api.startSignIn.mockResolvedValue(
      signIn({ connectorId: "codex-sub", url: "https://auth.openai.com/codex/device", userCode: "1MRK-O9PDA" }),
    );
    api.loadSignIn.mockResolvedValueOnce(
      signIn({ connectorId: "codex-sub", url: "https://auth.openai.com/codex/device", userCode: "1MRK-O9PDA" }),
    );
    api.loadSignIn.mockResolvedValueOnce(signIn({ connectorId: "codex-sub", state: "done" }));
    const onDone = vi.fn();
    render(<SignInDialog row={{ id: "codex-sub", name: "Codex subscription" }} onClose={vi.fn()} onDone={onDone} pollMs={10} />);
    const code = await screen.findByRole("status", { name: "Enter this code on that page" });
    expect(code.textContent).toBe("1MRK-O9PDA");
    expect(code.classList.contains("signin-code")).toBe(true);
    expect(screen.getByRole("button", { name: "Copy code" })).toBeTruthy();
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
    expect(api.loadSignIn).toHaveBeenCalledWith("s1");
    expect(api.cancelSignIn).not.toHaveBeenCalled();
  });

  it("submits the code Anthropic shows", async () => {
    api.startSignIn.mockResolvedValue(signIn({ needsCode: true }));
    api.submitSignInCode.mockResolvedValue(signIn({ state: "done" }));
    const onDone = vi.fn();
    render(<SignInDialog row={claude} onClose={vi.fn()} onDone={onDone} />);
    const input = (await screen.findByLabelText("Paste the code")) as HTMLInputElement;
    expect(input.type).toBe("text");
    const submit = screen.getByRole("button", { name: "Submit" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    await userEvent.type(input, "  abc#xyz ");
    await userEvent.click(submit);
    expect(api.submitSignInCode).toHaveBeenCalledWith("s1", "abc#xyz");
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
  });

  it("shows why a login failed and retries", async () => {
    api.startSignIn.mockResolvedValueOnce(signIn({ state: "failed", url: null, error: "Login was refused" }));
    api.startSignIn.mockResolvedValueOnce(signIn({ id: "s2" }));
    render(<SignInDialog row={claude} onClose={vi.fn()} onDone={vi.fn()} />);
    expect((await screen.findByRole("alert")).textContent).toBe("Login was refused");
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(api.startSignIn).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole("link")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows a request that failed to start, with Retry", async () => {
    api.startSignIn.mockRejectedValueOnce(new Error("pi is not installed"));
    render(<SignInDialog row={claude} onClose={vi.fn()} onDone={vi.fn()} />);
    expect((await screen.findByRole("alert")).textContent).toBe("pi is not installed");
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("cancels from the Cancel button", async () => {
    api.startSignIn.mockResolvedValue(signIn());
    const onClose = vi.fn();
    render(<SignInDialog row={claude} onClose={onClose} onDone={vi.fn()} />);
    await screen.findByRole("link");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.cancelSignIn).toHaveBeenCalledWith("s1");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("cancels on Escape", async () => {
    api.startSignIn.mockResolvedValue(signIn());
    const onClose = vi.fn();
    render(<SignInDialog row={claude} onClose={onClose} onDone={vi.fn()} />);
    await screen.findByRole("link");
    // A native modal turns Escape into a `cancel` event; jsdom does not, so send it.
    act(() => {
      fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }));
    });
    expect(api.cancelSignIn).toHaveBeenCalledWith("s1");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("stops a live login when it unmounts", async () => {
    api.startSignIn.mockResolvedValue(signIn());
    const { unmount } = render(<SignInDialog row={claude} onClose={vi.fn()} onDone={vi.fn()} />);
    await screen.findByRole("link");
    unmount();
    expect(api.cancelSignIn).toHaveBeenCalledWith("s1");
  });
});
