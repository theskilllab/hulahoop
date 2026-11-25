import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import DiffView, { FOLD_LINES, type DiffComment } from "./DiffView";
import { filesFromGithub } from "./diff";

const files = filesFromGithub([
  {
    path: "src/backoff.ts",
    previousPath: null,
    status: "modified",
    additions: 1,
    deletions: 1,
    patch: "@@ -1,2 +1,2 @@\n keep\n-old line\n+new line",
  },
]);

const comment = (over: Partial<DiffComment>): DiffComment => ({
  id: "c1",
  path: "src/backoff.ts",
  line: 2,
  side: "RIGHT",
  body: "Use a constant here",
  authorLogin: "you",
  createdAt: new Date().toISOString(),
  pending: true,
  onlyInHulahoop: false,
  ...over,
});

describe("DiffView", () => {
  it("shows the file header with its counts and folds it", async () => {
    render(<DiffView files={files} />);
    const fold = screen.getByRole("button", { name: /src\/backoff\.ts/ });
    expect(fold.textContent).toContain("+1");
    expect(fold.textContent).toContain("−1");
    expect(screen.getByText("+new line")).toBeTruthy();
    await userEvent.click(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("+new line")).toBeNull();
  });

  it("offers no comment button without onAddComment", () => {
    render(<DiffView files={files} />);
    expect(screen.queryByRole("button", { name: /Comment on line/ })).toBeNull();
  });

  it("adds a comment anchored on the old side of a removed line", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    render(<DiffView files={files} onAddComment={onAdd} />);
    await userEvent.click(screen.getByRole("button", { name: "Comment on removed line 2 of src/backoff.ts" }));
    const box = screen.getByRole("textbox", { name: "Comment on removed line 2 of src/backoff.ts" });
    const commentButton = screen.getByRole("button", { name: "Comment" });
    expect((commentButton as HTMLButtonElement).disabled).toBe(true);
    await userEvent.type(box, "Why the change?");
    await userEvent.click(commentButton);
    expect(onAdd).toHaveBeenCalledWith({ path: "src/backoff.ts", line: 2, side: "LEFT" }, "Why the change?");
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("anchors added and context lines on the RIGHT side", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    render(<DiffView files={files} onAddComment={onAdd} />);
    await userEvent.click(screen.getByRole("button", { name: "Comment on line 2 of src/backoff.ts" }));
    await userEvent.type(screen.getByRole("textbox"), "ok");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(onAdd).toHaveBeenCalledWith({ path: "src/backoff.ts", line: 2, side: "RIGHT" }, "ok");
    await userEvent.click(screen.getByRole("button", { name: "Comment on line 1 of src/backoff.ts" }));
    await userEvent.type(screen.getByRole("textbox"), "ctx");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect(onAdd).toHaveBeenLastCalledWith({ path: "src/backoff.ts", line: 1, side: "RIGHT" }, "ctx");
  });

  it("cancels with Escape and keeps the box open when saving fails", async () => {
    const onAdd = vi.fn().mockRejectedValue(new Error("GitHub said no"));
    render(<DiffView files={files} onAddComment={onAdd} />);
    const [first] = screen.getAllByRole("button", { name: /Comment on line/ });
    await userEvent.click(first);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.activeElement).toBe(first);

    await userEvent.click(first);
    await userEvent.type(screen.getByRole("textbox"), "x");
    await userEvent.click(screen.getByRole("button", { name: "Comment" }));
    expect((await screen.findByRole("alert")).textContent).toBe("GitHub said no");
    expect(screen.getByRole("textbox")).toBeTruthy();
  });

  it("shows saved comments under their line, with Only in Hulahoop and delete for pending ones", async () => {
    const onDelete = vi.fn().mockResolvedValue(undefined);
    render(
      <DiffView
        files={files}
        comments={[
          comment({ onlyInHulahoop: true }),
          comment({ id: "c2", side: "LEFT", body: "Posted already", authorLogin: "maya", pending: false }),
        ]}
        onAddComment={vi.fn()}
        onDeleteComment={onDelete}
      />,
    );
    const mine = screen.getByText("Use a constant here").closest(".diff-cmt") as HTMLElement;
    expect(mine.className).toContain("pending");
    expect(within(mine).getByText("You")).toBeTruthy();
    expect(within(mine).getByText("Only in Hulahoop")).toBeTruthy();

    const posted = screen.getByText("Posted already").closest(".diff-cmt") as HTMLElement;
    expect(within(posted).getByText("maya")).toBeTruthy();
    expect(within(posted).queryByText("Only in Hulahoop")).toBeNull();
    expect(within(posted).queryByRole("button", { name: "Delete" })).toBeNull();

    // The added line's comment sits after the added line, the removed line's after the removed one.
    const rows = Array.from(document.querySelectorAll(".diff-line, .diff-cmt"));
    const at = (element: Element) => rows.indexOf(element);
    expect(at(posted)).toBe(at(screen.getByText("-old line").closest(".diff-line")!) + 1);
    expect(at(mine)).toBe(at(screen.getByText("+new line").closest(".diff-line")!) + 1);

    await userEvent.click(within(mine).getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledWith("c1");
  });

  it("labels a pending comment that will go to GitHub", () => {
    render(<DiffView files={files} comments={[comment({})]} />);
    expect(screen.getByText("Pending")).toBeTruthy();
  });

  it("says when a file is binary", () => {
    render(<DiffView files={filesFromGithub([{ path: "a.png", previousPath: null, status: "added", additions: 0, deletions: 0, patch: null }])} />);
    expect(screen.getByText(/Binary file/)).toBeTruthy();
    expect(screen.getByText("Added")).toBeTruthy();
  });

  it("places a LEFT comment on a context line under that line", () => {
    render(<DiffView files={files} comments={[comment({ id: "c3", line: 1, side: "LEFT", body: "On the kept line", pending: false })]} />);
    const rows = Array.from(document.querySelectorAll(".diff-line, .diff-cmt"));
    const note = screen.getByText("On the kept line").closest(".diff-cmt")!;
    const kept = Array.from(document.querySelectorAll(".diff-line")).find((row) => row.textContent?.endsWith(" keep"))!;
    expect(rows.indexOf(note)).toBe(rows.indexOf(kept) + 1);
    expect(screen.queryByText("On lines outside this diff")).toBeNull();
  });

  it("makes each file one Tab stop and moves between lines with the arrow keys", async () => {
    render(<DiffView files={files} onAddComment={vi.fn()} />);
    const buttons = within(screen.getByRole("group", { name: "Changes in src/backoff.ts" })).getAllByRole("button");
    expect(buttons.map((button) => button.tabIndex)).toEqual([0, -1, -1]);
    buttons[0].focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(buttons[1]);
    expect(buttons.map((button) => button.tabIndex)).toEqual([-1, 0, -1]);
    await userEvent.keyboard("{End}");
    expect(document.activeElement).toBe(buttons[2]);
    await userEvent.keyboard("{Home}");
    expect(document.activeElement).toBe(buttons[0]);
    expect(document.querySelector('[role="table"], [role="row"], [role="cell"]')).toBeNull();
  });

  it("folds a very large file until asked", async () => {
    const lines = Array.from({ length: FOLD_LINES + 1 }, (_, index) => `+line ${index}`);
    const big = filesFromGithub([
      { path: "big.ts", previousPath: null, status: "added", additions: lines.length, deletions: 0, patch: `@@ -0,0 +1,${lines.length} @@\n${lines.join("\n")}` },
    ]);
    render(<DiffView files={big} />);
    expect(screen.queryByText("+line 0")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: `Show diff (${FOLD_LINES + 2} lines)` }));
    expect(screen.getByText("+line 0")).toBeTruthy();
  });

  it("keeps an open comment box on its line when a comment appears above it", async () => {
    const { rerender } = render(<DiffView files={files} onAddComment={vi.fn()} />);
    await userEvent.click(screen.getByRole("button", { name: "Comment on line 2 of src/backoff.ts" }));
    await userEvent.type(screen.getByRole("textbox", { name: "Comment on line 2 of src/backoff.ts" }), "draft");
    rerender(<DiffView files={files} comments={[comment({ id: "c4", line: 1, body: "Above", pending: false })]} onAddComment={vi.fn()} />);
    const box = screen.getByRole("textbox", { name: "Comment on line 2 of src/backoff.ts" }) as HTMLTextAreaElement;
    expect(box.value).toBe("draft");
    const rows = Array.from(document.querySelectorAll(".diff-line, .diff-cmt"));
    expect(rows.indexOf(box.closest(".diff-cmt")!)).toBe(rows.indexOf(screen.getByText("+new line").closest(".diff-line")!) + 1);
  });
});
