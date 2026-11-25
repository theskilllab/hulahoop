import { describe, expect, it } from "vitest";
import { anchorOf, filesFromGithub, parseGitDiff, parsePatch, totals, unquote } from "./diff";

const GIT = [
  "diff --git a/src/deliver.ts b/src/deliver.ts",
  "index 1111111..2222222 100644",
  "--- a/src/deliver.ts",
  "+++ b/src/deliver.ts",
  "@@ -10,4 +10,5 @@ export function deliver() {",
  " const a = 1;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4;",
  "",
  " return a;",
  "diff --git a/src/new.ts b/src/new.ts",
  "new file mode 100644",
  "index 0000000..3333333",
  "--- /dev/null",
  "+++ b/src/new.ts",
  "@@ -0,0 +1,2 @@",
  "+export const x = 1;",
  "+export const y = 2;",
  "\\ No newline at end of file",
  "diff --git a/old name.ts b/new name.ts",
  "similarity index 90%",
  "rename from old name.ts",
  "rename to new name.ts",
  "diff --git a/logo.png b/logo.png",
  "index 4444444..5555555 100644",
  "Binary files a/logo.png and b/logo.png differ",
  "diff --git a/gone.ts b/gone.ts",
  "deleted file mode 100644",
  "--- a/gone.ts",
  "+++ /dev/null",
  "@@ -1 +0,0 @@",
  "-bye",
  "",
].join("\n");

describe("parseGitDiff", () => {
  const files = parseGitDiff(GIT);

  it("finds every file in order, with its status", () => {
    expect(files.map((file) => [file.path, file.status])).toEqual([
      ["src/deliver.ts", "modified"],
      ["src/new.ts", "added"],
      ["new name.ts", "renamed"],
      ["logo.png", "modified"],
      ["gone.ts", "removed"],
    ]);
  });

  it("numbers old and new lines from the hunk header", () => {
    const lines = files[0].lines;
    expect(lines[0]).toMatchObject({ kind: "hunk", text: "@@ -10,4 +10,5 @@ export function deliver() {" });
    expect(lines[1]).toMatchObject({ kind: "context", text: "const a = 1;", oldLine: 10, newLine: 10 });
    expect(lines[2]).toMatchObject({ kind: "del", text: "const b = 2;", oldLine: 11, newLine: null });
    expect(lines[3]).toMatchObject({ kind: "add", text: "const b = 3;", oldLine: null, newLine: 11 });
    expect(lines[4]).toMatchObject({ kind: "add", newLine: 12 });
    // A blank context line whose leading space was stripped still counts.
    expect(lines[5]).toMatchObject({ kind: "context", text: "", oldLine: 12, newLine: 13 });
    expect(lines[6]).toMatchObject({ kind: "context", text: "return a;", oldLine: 13, newLine: 14 });
    expect(lines).toHaveLength(7);
    expect(files[0]).toMatchObject({ additions: 2, deletions: 1 });
  });

  it("keeps a no-newline note and counts an added file", () => {
    expect(files[1]).toMatchObject({ additions: 2, deletions: 0 });
    expect(files[1].lines.at(-1)).toMatchObject({ kind: "note", text: "No newline at end of file" });
  });

  it("records a rename's old path, with spaces, and no lines", () => {
    expect(files[2]).toMatchObject({ previousPath: "old name.ts", lines: [], binary: false });
  });

  it("marks a binary file", () => {
    expect(files[3]).toMatchObject({ binary: true, lines: [] });
  });

  it("names a deleted file by its old path", () => {
    expect(files[4]).toMatchObject({ path: "gone.ts", deletions: 1 });
    expect(files[4].lines[1]).toMatchObject({ kind: "del", oldLine: 1 });
  });

  it("totals the diff", () => {
    expect(totals(files)).toEqual({ additions: 4, deletions: 2 });
  });

  it("reads nothing from an empty diff", () => {
    expect(parseGitDiff("")).toEqual([]);
  });
});

describe("parsePatch (GitHub's hunks)", () => {
  const patch = "@@ -1,3 +1,3 @@\n line one\n-line two\n+line 2\n line three\n@@ -20,2 +20,3 @@ fn x\n a\n+b\n c";

  it("reads several hunks with their own numbering", () => {
    const lines = parsePatch(patch);
    expect(lines.filter((line) => line.kind === "hunk")).toHaveLength(2);
    expect(lines[2]).toMatchObject({ kind: "del", oldLine: 2 });
    expect(lines[3]).toMatchObject({ kind: "add", newLine: 2 });
    expect(lines[6]).toMatchObject({ kind: "context", oldLine: 20, newLine: 20 });
    expect(lines[7]).toMatchObject({ kind: "add", newLine: 21 });
    expect(lines[8]).toMatchObject({ kind: "context", oldLine: 21, newLine: 22 });
  });

  it("reads a hunk header without counts as one line", () => {
    expect(parsePatch("@@ -3 +3 @@\n-a\n+b")).toMatchObject([
      { kind: "hunk" },
      { kind: "del", oldLine: 3 },
      { kind: "add", newLine: 3 },
    ]);
  });
});

describe("anchorOf", () => {
  const [, context, del, add] = parsePatch("@@ -5,2 +5,2 @@\n same\n-old\n+new");

  it("anchors added and context lines on the new side", () => {
    expect(anchorOf(context)).toEqual({ line: 5, side: "RIGHT" });
    expect(anchorOf(add)).toEqual({ line: 6, side: "RIGHT" });
  });

  it("anchors removed lines on the old side", () => {
    expect(anchorOf(del)).toEqual({ line: 6, side: "LEFT" });
  });

  it("gives a hunk header no anchor", () => {
    expect(anchorOf(parsePatch("@@ -1 +1 @@\n x")[0])).toBeNull();
  });
});

describe("filesFromGithub", () => {
  it("keeps GitHub's counts and treats a missing patch as binary", () => {
    const files = filesFromGithub([
      { path: "a.ts", previousPath: null, status: "modified", additions: 1, deletions: 1, patch: "@@ -1 +1 @@\n-a\n+b" },
      { path: "b.png", previousPath: "c.png", status: "renamed", additions: 0, deletions: 0, patch: null },
      { path: "d.ts", previousPath: null, status: "copied", additions: 3, deletions: 0, patch: "@@ -0,0 +1 @@\n+d" },
      { path: "e.ts", previousPath: null, status: "changed", additions: 2, deletions: 0, patch: "@@ -0,0 +1 @@\n+e" },
    ]);
    expect(files[0].lines).toHaveLength(3);
    expect(files[1]).toMatchObject({ binary: true, status: "renamed", previousPath: "c.png" });
    expect(files[2]).toMatchObject({ status: "copied", additions: 3 });
    // A status GitHub may add later reads as modified.
    expect(files[3]).toMatchObject({ status: "modified", additions: 2 });
  });
});

describe("parseGitDiff edge cases", () => {
  it("reads a diff with CRLF line endings", () => {
    const files = parseGitDiff(GIT.replace(/\n/g, "\r\n"));
    expect(files.map((file) => file.path)).toEqual(["src/deliver.ts", "src/new.ts", "new name.ts", "logo.png", "gone.ts"]);
    expect(files[0].lines.every((line) => !line.text.endsWith("\r"))).toBe(true);
    expect(files[0]).toMatchObject({ additions: 2, deletions: 1 });
  });

  it("reads a copy as copied, with the path it came from", () => {
    const [file] = parseGitDiff(
      ["diff --git a/src/a.ts b/src/b.ts", "similarity index 100%", "copy from src/a.ts", "copy to src/b.ts", ""].join("\n"),
    );
    expect(file).toMatchObject({ path: "src/b.ts", previousPath: "src/a.ts", status: "copied" });
  });

  it("unquotes paths git quoted, including escapes and UTF-8 bytes", () => {
    const [file] = parseGitDiff(
      [
        'diff --git "a/caf\\303\\251 \\"x\\".ts" "b/caf\\303\\251 \\"y\\".ts"',
        "similarity index 95%",
        'rename from "caf\\303\\251 \\"x\\".ts"',
        'rename to "caf\\303\\251 \\"y\\".ts"',
        "",
      ].join("\n"),
    );
    expect(file).toMatchObject({ path: 'café "y".ts', previousPath: 'café "x".ts', status: "renamed" });
    expect(unquote('"tab\\there"')).toBe("tab\there");
    expect(unquote("plain.ts")).toBe("plain.ts");
  });

  it("keeps a no-newline note in the middle of a hunk and goes on counting", () => {
    const lines = parsePatch(["@@ -1,2 +1,2 @@", "-old", "\\ No newline at end of file", "+new", "+more"].join("\n"));
    expect(lines.map((line) => line.kind)).toEqual(["hunk", "del", "note", "add", "add"]);
    expect(lines[2].text).toBe("No newline at end of file");
    expect(lines[3]).toMatchObject({ newLine: 1 });
    expect(lines[4]).toMatchObject({ newLine: 2 });
  });
});
