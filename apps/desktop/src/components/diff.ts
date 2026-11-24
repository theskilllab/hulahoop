/**
 * Unified diffs, parsed into rows a viewer can draw. Two inputs reach it: a
 * run's working diff (`jj diff --git`, several files, each under a
 * `diff --git` header) and GitHub's per-file `patch`, which is the hunks alone.
 */

export type DiffLineKind = "hunk" | "add" | "del" | "context" | "note";

export interface DiffLine {
  kind: DiffLineKind;
  /** The line as written, without its `+`/`-`/space prefix. A hunk keeps its whole header. */
  text: string;
  /** Line number in the old file: set for removed and context lines. */
  oldLine: number | null;
  /** Line number in the new file: set for added and context lines. */
  newLine: number | null;
}

export type DiffFileStatus = "added" | "removed" | "modified" | "renamed" | "copied";

const STATUSES: DiffFileStatus[] = ["added", "removed", "modified", "renamed", "copied"];

export interface DiffFile {
  path: string;
  /** The old path of a renamed file, or the source of a copied one. */
  previousPath: string | null;
  status: DiffFileStatus;
  /** A binary or too-large file: there are no lines to show. */
  binary: boolean;
  additions: number;
  deletions: number;
  lines: DiffLine[];
}

export type DiffSide = "LEFT" | "RIGHT";

/** Where a comment on this line is anchored, the way GitHub counts it: the new
 *  side for added and context lines, the old side for removed ones. */
export function anchorOf(line: DiffLine): { line: number; side: DiffSide } | null {
  if (line.kind === "del" && line.oldLine !== null) return { line: line.oldLine, side: "LEFT" };
  if ((line.kind === "add" || line.kind === "context") && line.newLine !== null) return { line: line.newLine, side: "RIGHT" };
  return null;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** Hunks alone, as GitHub returns them per file. */
export function parsePatch(patch: string): DiffLine[] {
  return parseHunks(patch.replace(/\r\n/g, "\n").split("\n"), 0).lines;
}

/**
 * Reads hunks from `start` until a line that cannot belong to one. Counts from
 * the hunk header decide where a hunk ends, so a blank context line whose
 * trailing space was stripped still counts, and the next file's header does not.
 */
function parseHunks(rows: string[], start: number): { lines: DiffLine[]; next: number } {
  const lines: DiffLine[] = [];
  let oldLine = 0;
  let newLine = 0;
  let oldLeft = 0;
  let newLeft = 0;
  let index = start;
  for (; index < rows.length; index += 1) {
    const row = rows[index];
    const header = HUNK.exec(row);
    if (header) {
      oldLine = Number(header[1]);
      oldLeft = header[2] === undefined ? 1 : Number(header[2]);
      newLine = Number(header[3]);
      newLeft = header[4] === undefined ? 1 : Number(header[4]);
      lines.push({ kind: "hunk", text: row, oldLine: null, newLine: null });
      continue;
    }
    if (row.startsWith("\\")) {
      // "\ No newline at end of file"
      lines.push({ kind: "note", text: row.slice(1).trim(), oldLine: null, newLine: null });
      continue;
    }
    if (oldLeft <= 0 && newLeft <= 0) break;
    const mark = row[0];
    if (mark === "+") {
      lines.push({ kind: "add", text: row.slice(1), oldLine: null, newLine });
      newLine += 1;
      newLeft -= 1;
    } else if (mark === "-") {
      lines.push({ kind: "del", text: row.slice(1), oldLine, newLine: null });
      oldLine += 1;
      oldLeft -= 1;
    } else if (mark === " " || row === "") {
      if (row === "" && index === rows.length - 1) break;
      lines.push({ kind: "context", text: row.slice(1), oldLine, newLine });
      oldLine += 1;
      newLine += 1;
      oldLeft -= 1;
      newLeft -= 1;
    } else {
      break;
    }
  }
  return { lines, next: index };
}

function tally(lines: DiffLine[]): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const line of lines) {
    if (line.kind === "add") additions += 1;
    else if (line.kind === "del") deletions += 1;
  }
  return { additions, deletions };
}

const ESCAPES: Record<string, number> = { a: 7, b: 8, t: 9, n: 10, v: 11, f: 12, r: 13, '"': 34, "\\": 92 };

/**
 * Git quotes a path with unusual bytes C-style: `"caf\303\251.txt"`. The octal
 * escapes are UTF-8 bytes, so they decode together.
 */
export function unquote(path: string): string {
  if (!(path.length >= 2 && path.startsWith('"') && path.endsWith('"'))) return path;
  const inner = path.slice(1, -1);
  const bytes: number[] = [];
  const encoder = new TextEncoder();
  for (let index = 0; index < inner.length; index += 1) {
    const char = inner[index];
    if (char !== "\\" || index === inner.length - 1) {
      bytes.push(...encoder.encode(char));
      continue;
    }
    const octal = /^[0-7]{3}/.exec(inner.slice(index + 1, index + 4));
    if (octal) {
      bytes.push(parseInt(octal[0], 8));
      index += 3;
    } else {
      const next = inner[index + 1];
      bytes.push(...(ESCAPES[next] !== undefined ? [ESCAPES[next]] : encoder.encode(next)));
      index += 1;
    }
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}

function stripPrefix(path: string): string {
  const plain = unquote(path.trim());
  return plain.replace(/^[ab]\//, "");
}

/** `diff --git a/x b/x`: when both halves are the same path, spaces in it are no problem. */
function headerPaths(rest: string): { from: string; to: string } {
  const length = (rest.length - 5) / 2;
  if (Number.isInteger(length) && length > 0) {
    const from = rest.slice(2, 2 + length);
    if (rest === `a/${from} b/${from}`) return { from, to: from };
  }
  const split = rest.match(/^("?a\/.+?"?) ("?b\/.+"?)$/);
  if (split) return { from: stripPrefix(split[1]), to: stripPrefix(split[2]) };
  return { from: rest, to: rest };
}

/** A whole `git diff` (or `jj diff --git`): one entry per file, in order. */
export function parseGitDiff(text: string): DiffFile[] {
  const rows = text.replace(/\r\n/g, "\n").split("\n");
  const files: DiffFile[] = [];
  let index = 0;
  while (index < rows.length) {
    const row = rows[index];
    if (!row.startsWith("diff --git ")) {
      index += 1;
      continue;
    }
    const paths = headerPaths(row.slice("diff --git ".length));
    let from: string | null = paths.from;
    let to: string | null = paths.to;
    let status: DiffFileStatus = "modified";
    let binary = false;
    index += 1;
    // Extended headers, up to the first hunk or the next file.
    while (index < rows.length && !rows[index].startsWith("diff --git ") && !rows[index].startsWith("@@")) {
      const meta = rows[index];
      if (meta.startsWith("new file mode")) status = "added";
      else if (meta.startsWith("deleted file mode")) status = "removed";
      else if (meta.startsWith("rename from ")) {
        from = unquote(meta.slice("rename from ".length));
        status = "renamed";
      } else if (meta.startsWith("rename to ")) {
        to = unquote(meta.slice("rename to ".length));
        status = "renamed";
      } else if (meta.startsWith("copy from ")) {
        from = unquote(meta.slice("copy from ".length));
        status = "copied";
      } else if (meta.startsWith("copy to ")) {
        to = unquote(meta.slice("copy to ".length));
        status = "copied";
      } else if (meta.startsWith("--- ")) {
        const value = meta.slice(4);
        if (value === "/dev/null") status = "added";
        else from = stripPrefix(value);
      } else if (meta.startsWith("+++ ")) {
        const value = meta.slice(4);
        if (value === "/dev/null") status = "removed";
        else to = stripPrefix(value);
      } else if (meta.startsWith("Binary files") || meta.startsWith("GIT binary patch")) {
        binary = true;
      }
      index += 1;
    }
    const parsed = parseHunks(rows, index);
    index = parsed.next;
    const path = status === "removed" ? from ?? to ?? "" : to ?? from ?? "";
    files.push({
      path,
      previousPath: (status === "renamed" || status === "copied") && from !== to ? from : null,
      status,
      binary,
      ...tally(parsed.lines),
      lines: parsed.lines,
    });
  }
  return files;
}

/** GitHub's file list: the counts are GitHub's, the lines come from its patch. */
export function filesFromGithub(
  files: { path: string; previousPath: string | null; status: string; additions: number; deletions: number; patch: string | null }[],
): DiffFile[] {
  return files.map((file) => ({
    path: file.path,
    previousPath: file.previousPath,
    status: (STATUSES as string[]).includes(file.status) ? (file.status as DiffFileStatus) : "modified",
    binary: file.patch === null,
    additions: file.additions,
    deletions: file.deletions,
    lines: file.patch === null ? [] : parsePatch(file.patch),
  }));
}

export function totals(files: DiffFile[]): { additions: number; deletions: number } {
  return files.reduce(
    (sum, file) => ({ additions: sum.additions + file.additions, deletions: sum.deletions + file.deletions }),
    { additions: 0, deletions: 0 },
  );
}
