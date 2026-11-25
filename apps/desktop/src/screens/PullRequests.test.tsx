import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PullRequest, PullRequestDetail } from "../api";

const api = vi.hoisted(() => ({
  loadPullRequests: vi.fn(),
  loadPullRequestSyncStatus: vi.fn(),
  loadGithubAccount: vi.fn(),
  loadConnections: vi.fn(),
  loadPullRequest: vi.fn(),
  syncPullRequests: vi.fn(),
  addPullRequestComment: vi.fn(),
  deletePullRequestComment: vi.fn(),
  reviewPullRequest: vi.fn(),
  mergePullRequest: vi.fn(),
  deletePullRequestBranch: vi.fn(),
}));

vi.mock("../api", () => api);
vi.mock("../ws", () => ({
  useRunDiff: () => ({ patch: "", live: true, updatedAt: null, error: null, reconnecting: false }),
}));

import PullRequests from "./PullRequests";

const pr = (over: Partial<PullRequest> = {}): PullRequest => ({
  id: "repo1#12",
  repoId: "repo1",
  repoName: "northwind/relay-api",
  number: 12,
  title: "Add retry with backoff to webhook delivery",
  url: "https://github.com/northwind/relay-api/pull/12",
  state: "open",
  draft: false,
  merged: false,
  authorLogin: "ada-bot",
  authorAvatarUrl: null,
  byAgent: true,
  headRef: "hul-7-retry",
  headSha: "abc",
  baseRef: "main",
  additions: 3,
  deletions: 1,
  changedFiles: 1,
  reviewState: "REVIEW_REQUESTED",
  reviewRequestedFromMe: true,
  ciState: "PASSING",
  mergeable: "CLEAN",
  createdAt: new Date(Date.now() - 12 * 60_000).toISOString(),
  updatedAt: new Date(Date.now() - 12 * 60_000).toISOString(),
  mergedAt: null,
  issue: { id: "issue-7", key: "HUL-7", title: "Retry webhooks", status: "in_review" },
  liveRun: null,
  ...over,
});

const detail = (over: Partial<PullRequestDetail> = {}): PullRequestDetail => ({
  pullRequest: pr(),
  body: null,
  files: [
    { path: "src/deliver.ts", previousPath: null, status: "modified", additions: 1, deletions: 1, patch: "@@ -1,2 +1,2 @@\n keep\n-old\n+new" },
  ],
  comments: [],
  reviews: [],
  checks: [],
  mergeMethods: ["MERGE", "SQUASH", "REBASE"],
  lastMergeMethod: "SQUASH",
  canReview: true,
  reviewBlockedReason: null,
  canMerge: true,
  mergeBlockedReason: null,
  canDeleteBranch: false,
  ...over,
});

const account = { login: "rez", name: "Rez", avatarUrl: null, tokenHint: "github_pat_…a1B2", validatedAt: new Date().toISOString(), problems: [] };

function props() {
  return { onOpenTask: vi.fn(), onOpenSettings: vi.fn(), onOpenConnectors: vi.fn(), onChanged: vi.fn() };
}

async function openFirst() {
  await userEvent.click(await screen.findByRole("button", { name: /Add retry with backoff/ }));
  await screen.findByText("src/deliver.ts");
}

beforeEach(() => {
  api.loadPullRequests.mockResolvedValue([pr()]);
  api.loadPullRequestSyncStatus.mockResolvedValue({ syncedAt: new Date().toISOString(), problems: [] });
  api.loadGithubAccount.mockResolvedValue(account);
  api.loadPullRequest.mockResolvedValue(detail());
  api.loadConnections.mockResolvedValue([{ id: "github", connected: true }]);
  api.syncPullRequests.mockResolvedValue(undefined);
});

/** A promise the test settles when it chooses. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** Sync with GitHub, which reloads the list; a moved updatedAt reloads the open pull request. */
async function syncWithMoved(over: Partial<PullRequest> = {}) {
  api.loadPullRequests.mockResolvedValue([pr({ updatedAt: new Date().toISOString(), ...over })]);
  await userEvent.click(screen.getByRole("button", { name: /Sync/ }));
}

describe("PullRequests list", () => {
  it("lists the open ones with their facts, and asks again for another filter", async () => {
    render(<PullRequests {...props()} />);
    const row = await screen.findByRole("button", { name: /Add retry with backoff/ });
    expect(api.loadPullRequests).toHaveBeenCalledWith("OPEN");
    expect(row.textContent).toContain("#12");
    expect(row.textContent).toContain("relay-api");
    expect(row.textContent).toContain("ada-bot (agent)");
    expect(row.textContent).toContain("12 min");
    expect(within(row).getByText("Review requested")).toBeTruthy();
    expect(within(row).getByText("CI passing")).toBeTruthy();

    const filters = screen.getByRole("group", { name: "Filter" });
    expect(within(filters).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "Open",
      "Needs my review",
      "Mine",
      "By agents",
      "Failing CI",
      "Merged",
    ]);
    expect(within(filters).getByRole("button", { name: "Open" }).getAttribute("aria-pressed")).toBe("true");

    api.loadPullRequests.mockResolvedValue([]);
    await userEvent.click(within(filters).getByRole("button", { name: "Merged" }));
    expect(api.loadPullRequests).toHaveBeenLastCalledWith("MERGED");
    expect(within(filters).getByRole("button", { name: "Merged" }).getAttribute("aria-pressed")).toBe("true");
    expect(await screen.findByText("No pull requests here")).toBeTruthy();
  });

  it("asks to select one until one is selected", async () => {
    render(<PullRequests {...props()} />);
    expect(await screen.findByText("Select a pull request")).toBeTruthy();
    await openFirst();
    expect(api.loadPullRequest).toHaveBeenCalledWith("repo1", 12);
    expect(screen.getByRole("button", { name: /Add retry with backoff/ }).getAttribute("aria-current")).toBe("true");
    expect(screen.getByText("Files changed · 1")).toBeTruthy();
    expect(screen.queryByText("Select a pull request")).toBeNull();
  });

  it("points to Settings when GitHub is not reachable at all", async () => {
    api.loadPullRequests.mockResolvedValue([]);
    api.loadGithubAccount.mockResolvedValue(null);
    api.loadPullRequestSyncStatus.mockResolvedValue({ syncedAt: null, problems: ["no GitHub access for northwind/relay-api"] });
    const handlers = props();
    render(<PullRequests {...handlers} />);
    expect(await screen.findByText("GitHub isn’t connected")).toBeTruthy();
    expect(screen.getByText("no GitHub access for northwind/relay-api")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Settings → Personal/ }));
    expect(handlers.onOpenSettings).toHaveBeenCalled();
  });

  it("opens the linked task", async () => {
    const handlers = props();
    render(<PullRequests {...handlers} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "HUL-7" }));
    expect(handlers.onOpenTask).toHaveBeenCalledWith("issue-7");
  });
});

describe("Merge", () => {
  it("is disabled with the server's reason", async () => {
    api.loadPullRequest.mockResolvedValue(detail({ canMerge: false, mergeBlockedReason: "Waiting for an approving review" }));
    render(<PullRequests {...props()} />);
    await openFirst();
    const merge = screen.getByRole("button", { name: "Squash and merge" }) as HTMLButtonElement;
    expect(merge.disabled).toBe(true);
    expect(merge.title).toBe("Waiting for an approving review");
    expect(screen.getByText("Waiting for an approving review")).toBeTruthy();
  });

  it("disables review and merge without a token and says where to add one", async () => {
    const why = "Add your GitHub token in Settings → Personal";
    api.loadGithubAccount.mockResolvedValue(null);
    api.loadPullRequest.mockResolvedValue(detail({ canReview: false, reviewBlockedReason: why, canMerge: false, mergeBlockedReason: why }));
    const handlers = props();
    render(<PullRequests {...handlers} />);
    await openFirst();
    expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Request changes" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(why)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Open Settings" }));
    expect(handlers.onOpenSettings).toHaveBeenCalled();
  });

  it("preselects the last method, lets another be chosen, and merges with it", async () => {
    api.mergePullRequest.mockResolvedValue(
      pr({ merged: true, state: "closed", reviewState: "MERGED", issue: { id: "i7", key: "HUL-7", title: "Task", status: "done" } }),
    );
    const handlers = props();
    render(<PullRequests {...handlers} />);
    await openFirst();
    const more = screen.getByRole("button", { name: "Merge method" });
    await userEvent.click(more);
    const menu = screen.getByRole("menu", { name: "Merge method" });
    expect(within(menu).getByRole("menuitemradio", { name: /Squash and merge/ }).getAttribute("aria-checked")).toBe("true");
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();

    await userEvent.click(more);
    await userEvent.click(screen.getByRole("menuitemradio", { name: /Rebase and merge/ }));
    await userEvent.click(screen.getByRole("button", { name: "Rebase and merge" }));
    expect(api.mergePullRequest).toHaveBeenCalledWith("repo1", 12, "REBASE", "abc");
    expect((await screen.findByRole("status")).textContent).toBe("Merged #12 and closed HUL-7");
    expect(handlers.onChanged).toHaveBeenCalled();
  });

  it("says Merged #N when no task is linked, and shows GitHub's reason on failure", async () => {
    api.loadPullRequests.mockResolvedValue([pr({ issue: null })]);
    api.loadPullRequest.mockResolvedValue(detail({ pullRequest: pr({ issue: null }) }));
    api.mergePullRequest.mockRejectedValueOnce(new Error("Base branch was modified"));
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Squash and merge" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Base branch was modified");

    api.mergePullRequest.mockResolvedValue(pr({ issue: null, merged: true }));
    await userEvent.click(screen.getByRole("button", { name: "Squash and merge" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Merged #12"));
  });

  it("does not claim the task closed when the merge left it open", async () => {
    api.mergePullRequest.mockResolvedValue(
      pr({ merged: true, state: "closed", issue: { id: "i7", key: "HUL-7", title: "Task", status: "in_review" } }),
    );
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Squash and merge" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Merged #12"));
  });
});

describe("Delete branch", () => {
  it("offers to delete the branch only once merged and while it is still there", async () => {
    const merged = pr({ merged: true, state: "closed", reviewState: "MERGED" });
    api.loadPullRequests.mockResolvedValue([merged]);
    api.loadPullRequest.mockResolvedValue(detail({ pullRequest: merged, canDeleteBranch: true }));
    api.deletePullRequestBranch.mockResolvedValue(detail({ pullRequest: merged, canDeleteBranch: false }));
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Delete branch" }));
    expect(api.deletePullRequestBranch).toHaveBeenCalledWith("repo1", 12);
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("Deleted branch hul-7-retry"));
    expect(screen.queryByRole("button", { name: "Delete branch" })).toBeNull();
  });

  it("does not offer it on an open pull request", async () => {
    api.loadPullRequest.mockResolvedValue(detail({ canDeleteBranch: true }));
    render(<PullRequests {...props()} />);
    await openFirst();
    expect(screen.queryByRole("button", { name: "Delete branch" })).toBeNull();
  });
});

describe("Review", () => {
  it("asks for a note or a line comment before requesting changes", async () => {
    api.reviewPullRequest.mockResolvedValue(detail());
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Request changes" }));
    expect(screen.getByText("(tip: click + beside a line to comment on it)")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Send feedback" }));
    expect(screen.getByRole("alert").textContent).toBe("Add a note, or comment on a line in the diff, so the change is clear.");
    expect(api.reviewPullRequest).not.toHaveBeenCalled();

    await userEvent.type(screen.getByRole("textbox", { name: /What should change/ }), "Cap the retries");
    await userEvent.click(screen.getByRole("button", { name: "Send feedback" }));
    expect(api.reviewPullRequest).toHaveBeenCalledWith("repo1", 12, "REQUEST_CHANGES", "Cap the retries");
    expect((await screen.findByRole("status")).textContent).toBe("Requested changes on #12");
  });

  it("sends pending line comments without a note and says so", async () => {
    const pending = (id: string, line: number) => ({
      id,
      path: "src/deliver.ts",
      line,
      side: "RIGHT" as const,
      body: `comment ${id}`,
      authorLogin: "rez",
      createdAt: new Date().toISOString(),
      pending: true,
      onlyInHulahoop: false,
    });
    api.loadPullRequest.mockResolvedValue(detail({ comments: [pending("a", 1), pending("b", 2)] }));
    api.reviewPullRequest.mockResolvedValue(detail());
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Request changes (2)" }));
    expect(screen.getByText("(2 line comments will be sent too)")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Send feedback" }));
    expect(api.reviewPullRequest).toHaveBeenCalledWith("repo1", 12, "REQUEST_CHANGES", null);
    expect((await screen.findByRole("status")).textContent).toBe("Requested changes on #12 · 2 comments");
  });

  it("approves in one click", async () => {
    api.reviewPullRequest.mockResolvedValue(detail({ pullRequest: pr({ reviewState: "APPROVED" }) }));
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Approve" }));
    expect(api.reviewPullRequest).toHaveBeenCalledWith("repo1", 12, "APPROVE", null);
    expect((await screen.findByRole("status")).textContent).toBe("Approved #12");
  });

  it("saves a line comment through the API", async () => {
    api.addPullRequestComment.mockResolvedValue({
      id: "n1",
      path: "src/deliver.ts",
      line: 2,
      side: "RIGHT",
      body: "nit",
      authorLogin: "rez",
      createdAt: new Date().toISOString(),
      pending: true,
      onlyInHulahoop: false,
    });
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Comment on line 2 of src/deliver.ts" }));
    await userEvent.type(screen.getByRole("textbox", { name: "Comment on line 2 of src/deliver.ts" }), "nit");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(api.addPullRequestComment).toHaveBeenCalledWith({ repoId: "repo1", number: 12, path: "src/deliver.ts", line: 2, side: "RIGHT", body: "nit" });
    expect(await screen.findByText("nit")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Request changes (1)" })).toBeTruthy();
  });
});

describe("Watch live", () => {
  it("shows the run's diff when the task has a live run", async () => {
    api.loadPullRequests.mockResolvedValue([pr({ liveRun: { id: "run-1", agentId: "a1", status: "running" } })]);
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Watch live" }));
    expect(screen.getByText("Live diff · 0")).toBeTruthy();
    expect(screen.getByText("Connecting…")).toBeTruthy();
  });
});

describe("Not connected", () => {
  it("waits for the first sync rather than saying GitHub isn't connected", async () => {
    api.loadPullRequests.mockResolvedValue([]);
    api.loadGithubAccount.mockResolvedValue(null);
    api.loadPullRequestSyncStatus.mockResolvedValue({ syncedAt: null, problems: [] });
    render(<PullRequests {...props()} />);
    expect(await screen.findByText("No pull requests here")).toBeTruthy();
    expect(screen.queryByText("GitHub isn’t connected")).toBeNull();
  });

  it("says so when there is no token and the GitHub App is not connected", async () => {
    api.loadPullRequests.mockResolvedValue([]);
    api.loadGithubAccount.mockResolvedValue(null);
    api.loadPullRequestSyncStatus.mockResolvedValue({ syncedAt: null, problems: [] });
    api.loadConnections.mockResolvedValue([{ id: "github", connected: false }]);
    render(<PullRequests {...props()} />);
    expect(await screen.findByText("GitHub isn’t connected")).toBeTruthy();
  });
});

describe("Sync", () => {
  it("says the list could not reload instead of claiming it is up to date", async () => {
    render(<PullRequests {...props()} />);
    await screen.findByRole("button", { name: /Add retry with backoff/ });
    api.loadPullRequests.mockRejectedValueOnce(new Error("server unreachable"));
    await userEvent.click(screen.getByRole("button", { name: /Sync/ }));
    await waitFor(() =>
      expect(screen.getAllByRole("alert").map((node) => node.textContent)).toContain(
        "Synced with GitHub, but the list could not reload",
      ),
    );
    expect(screen.queryByText("Pull requests are up to date")).toBeNull();
    // The rows already loaded stay on screen while syncing and after.
    expect(screen.getByRole("button", { name: /Add retry with backoff/ })).toBeTruthy();
  });
});

describe("Narrow layout", () => {
  it("shows the detail after a row is opened, and Back returns to the row", async () => {
    const { container } = render(<PullRequests {...props()} />);
    await openFirst();
    const split = container.querySelector(".prsplit") as HTMLElement;
    expect(split.classList.contains("showdetail")).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(split.classList.contains("showdetail")).toBe(false);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: /Add retry with backoff/ })));
  });
});

describe("Drafts", () => {
  it("can be opened on GitHub and watched, never reviewed or merged here", async () => {
    const draft = pr({ draft: true, liveRun: { id: "run-1", agentId: "a1", status: "running" } });
    api.loadPullRequests.mockResolvedValue([draft]);
    api.loadPullRequest.mockResolvedValue(
      detail({
        pullRequest: draft,
        canReview: false,
        reviewBlockedReason: "This pull request is still a draft",
        canMerge: false,
        mergeBlockedReason: "This pull request is still a draft",
      }),
    );
    render(<PullRequests {...props()} />);
    await openFirst();
    expect(screen.getByRole("link", { name: /Open on GitHub/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Watch live" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.queryByRole("button", { name: /Request changes/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Squash and merge" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Merge method" })).toBeNull();
  });
});

describe("Review, once", () => {
  it("sends feedback once however often Send is pressed while it is going", async () => {
    const sending = deferred<PullRequestDetail>();
    api.reviewPullRequest.mockReturnValue(sending.promise);
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Request changes" }));
    const note = screen.getByRole("textbox", { name: /What should change/ });
    await userEvent.type(note, "Cap the retries");
    const send = screen.getByRole("button", { name: "Send feedback" });
    // Two clicks in the same tick, then the keyboard shortcut.
    act(() => {
      fireEvent.click(send);
      fireEvent.click(send);
    });
    fireEvent.keyDown(note, { key: "Enter", ctrlKey: true });
    fireEvent.keyDown(note, { key: "Enter", metaKey: true });
    expect(api.reviewPullRequest).toHaveBeenCalledTimes(1);
    await act(async () => sending.resolve(detail()));
    expect((await screen.findByRole("status")).textContent).toBe("Requested changes on #12");
    expect(api.reviewPullRequest).toHaveBeenCalledTimes(1);
  });

  it("keeps the review's result when a slower load started before it lands later", async () => {
    const pending = {
      id: "a",
      path: "src/deliver.ts",
      line: 2,
      side: "RIGHT" as const,
      body: "comment a",
      authorLogin: "rez",
      createdAt: new Date().toISOString(),
      pending: true,
      onlyInHulahoop: false,
    };
    api.loadPullRequest.mockResolvedValue(detail({ comments: [pending] }));
    api.reviewPullRequest.mockResolvedValue(detail());
    render(<PullRequests {...props()} />);
    await openFirst();
    expect(screen.getByRole("button", { name: "Request changes (1)" })).toBeTruthy();

    const slow = deferred<PullRequestDetail>();
    api.loadPullRequest.mockReturnValueOnce(slow.promise);
    await syncWithMoved();
    await waitFor(() => expect(api.loadPullRequest).toHaveBeenCalledTimes(2));

    // From here on GitHub has the comment as sent.
    api.loadPullRequest.mockResolvedValue(detail());
    await userEvent.click(screen.getByRole("button", { name: "Request changes (1)" }));
    await userEvent.click(screen.getByRole("button", { name: "Send feedback" }));
    expect(await screen.findByRole("button", { name: "Request changes" })).toBeTruthy();

    // The old load still names the comment as pending; it must not bring it back.
    await act(async () => slow.resolve(detail({ comments: [pending] })));
    expect(screen.getByRole("button", { name: "Request changes" })).toBeTruthy();
    expect(screen.queryByText("comment a")).toBeNull();
  });
});

describe("Line comments across reloads", () => {
  it("keeps an open comment box, and what is typed, on its line when the pull request reloads", async () => {
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Comment on line 2 of src/deliver.ts" }));
    await userEvent.type(screen.getByRole("textbox", { name: "Comment on line 2 of src/deliver.ts" }), "half a thought");

    // GitHub now has a comment on line 1, which adds a block above the open box.
    api.loadPullRequest.mockResolvedValue(
      detail({
        comments: [
          {
            id: "gh1",
            path: "src/deliver.ts",
            line: 1,
            side: "RIGHT",
            body: "from GitHub",
            authorLogin: "ada",
            createdAt: new Date().toISOString(),
            pending: false,
            onlyInHulahoop: false,
          },
        ],
      }),
    );
    await syncWithMoved();
    expect(await screen.findByText("from GitHub")).toBeTruthy();
    const box = screen.getByRole("textbox", { name: "Comment on line 2 of src/deliver.ts" }) as HTMLTextAreaElement;
    expect(box.value).toBe("half a thought");
  });
});

describe("Merge menu keyboard", () => {
  it("moves between methods with the arrow keys and closes when focus leaves", async () => {
    render(<PullRequests {...props()} />);
    await openFirst();
    await userEvent.click(screen.getByRole("button", { name: "Merge method" }));
    const items = within(screen.getByRole("menu", { name: "Merge method" })).getAllByRole("menuitemradio");
    await waitFor(() => expect(document.activeElement).toBe(items[1]));
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(items[2]);
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(items[0]);
    await userEvent.keyboard("{End}");
    expect(document.activeElement).toBe(items[2]);
    await userEvent.keyboard("{Home}");
    expect(document.activeElement).toBe(items[0]);
    await userEvent.tab();
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

describe("CI line", () => {
  it("is not repeated when the merge reason already names CI", async () => {
    api.loadPullRequests.mockResolvedValue([pr({ ciState: "FAILING" })]);
    api.loadPullRequest.mockResolvedValue(
      detail({ pullRequest: pr({ ciState: "FAILING" }), canMerge: false, mergeBlockedReason: "CI is failing" }),
    );
    render(<PullRequests {...props()} />);
    await openFirst();
    expect(screen.getAllByText(/CI is failing/)).toHaveLength(1);
  });
});

describe("Conflicts", () => {
  const why = "This branch has conflicts with main; resolve them before merging";

  it("shows a chip in the list and a line in the detail, and the merge reason once", async () => {
    api.loadPullRequests.mockResolvedValue([pr({ mergeable: "CONFLICTING" })]);
    api.loadPullRequest.mockResolvedValue(
      detail({ pullRequest: pr({ mergeable: "CONFLICTING" }), canMerge: false, mergeBlockedReason: why }),
    );
    render(<PullRequests {...props()} />);
    const row = await screen.findByRole("button", { name: /Add retry with backoff/ });
    expect(within(row).getByText("Conflicts")).toBeTruthy();
    await openFirst();
    const line = screen.getByText("This branch has conflicts with main that must be resolved.");
    expect(line.getAttribute("role")).toBe("status");
    expect(screen.getByRole("link", { name: "Open on GitHub" })).toBeTruthy();
    const merge = screen.getByRole("button", { name: "Squash and merge" }) as HTMLButtonElement;
    expect(merge.disabled).toBe(true);
    expect(merge.title).toBe(why);
    expect(screen.queryByText(why)).toBeNull();
  });

  it("follows the live check once the detail has loaded", async () => {
    api.loadPullRequests.mockResolvedValue([pr({ mergeable: "CONFLICTING" })]);
    api.loadPullRequest.mockResolvedValue(detail({ pullRequest: pr({ mergeable: "CLEAN" }) }));
    render(<PullRequests {...props()} />);
    await openFirst();
    expect(screen.queryByText(/must be resolved/)).toBeNull();
  });

  it("says nothing for a merged pull request", async () => {
    const merged = pr({ merged: true, state: "closed", reviewState: "MERGED", mergeable: "CONFLICTING" });
    api.loadPullRequests.mockResolvedValue([merged]);
    api.loadPullRequest.mockResolvedValue(detail({ pullRequest: merged }));
    render(<PullRequests {...props()} />);
    const row = await screen.findByRole("button", { name: /Add retry with backoff/ });
    expect(within(row).queryByText("Conflicts")).toBeNull();
    await openFirst();
    expect(screen.queryByText(/must be resolved/)).toBeNull();
  });
});
