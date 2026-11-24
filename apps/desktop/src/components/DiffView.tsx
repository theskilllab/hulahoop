import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { anchorOf, type DiffFile, type DiffLine, type DiffSide } from "./diff";
import { fmtWhen } from "./TaskBits";
import Icon from "../Icon";

/** A comment on one line of one file. `PullRequestComment` is one. */
export interface DiffComment {
  id: string;
  path: string;
  line: number;
  side: DiffSide;
  body: string;
  authorLogin: string;
  createdAt: string;
  /** Saved in Hulahoop, not yet on GitHub: it goes with the next review. */
  pending: boolean;
  /** Pending, and there is no GitHub token to send it with. */
  onlyInHulahoop: boolean;
}

export interface CommentAnchor {
  path: string;
  line: number;
  side: DiffSide;
}

/** Files with more rows than this start folded: drawing thousands of rows nobody asked for costs frames. */
export const FOLD_LINES = 500;

const STATUS_LABEL: Record<string, string> = {
  added: "Added",
  removed: "Deleted",
  renamed: "Renamed",
  copied: "Copied",
  modified: "Modified",
};

const NO_COMMENTS: DiffComment[] = [];

const fileKey = (file: DiffFile) => `${file.previousPath ?? ""}→${file.path}`;

/**
 * Each row's React key: what it is and where it sits in the old and new file.
 * A reload that shifts rows keeps an open comment box on the line it was opened on.
 */
function rowKeys(lines: DiffLine[]): string[] {
  const seen = new Map<string, number>();
  return lines.map((line) => {
    const base = `${line.kind}:${line.oldLine ?? ""}:${line.newLine ?? ""}`;
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    return count ? `${base}#${count}` : base;
  });
}

interface Placed {
  /** The comments under each row, by row index. */
  byRow: DiffComment[][];
  /** Comments on lines the diff does not show. */
  outside: DiffComment[];
}

/** Puts each comment of one file under its line, in one pass over rows and comments. */
function place(file: DiffFile, comments: DiffComment[]): Placed {
  const byAnchor = new Map<string, DiffComment[]>();
  for (const comment of comments) {
    const key = `${comment.side}:${comment.line}`;
    const list = byAnchor.get(key);
    if (list) list.push(comment);
    else byAnchor.set(key, [comment]);
  }
  const shown = new Set<string>();
  const byRow = file.lines.map((line) => {
    const anchor = anchorOf(line);
    if (!anchor) return NO_COMMENTS;
    const here = [...(byAnchor.get(`${anchor.side}:${anchor.line}`) ?? [])];
    // GitHub can anchor a comment on a context line to its old side.
    if (line.kind === "context" && line.oldLine !== null) here.push(...(byAnchor.get(`LEFT:${line.oldLine}`) ?? []));
    if (here.length === 0) return NO_COMMENTS;
    here.forEach((comment) => shown.add(comment.id));
    return here;
  });
  return { byRow, outside: comments.filter((comment) => !shown.has(comment.id)) };
}

/** Arrow keys, Home and End move between the "+" buttons of one file, so the file is one Tab stop. */
function moveBetweenLines(event: KeyboardEvent<HTMLDivElement>) {
  const target = event.target as HTMLElement;
  if (!target.classList.contains("diff-cbtn")) return;
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>(".diff-cbtn"));
  const at = buttons.indexOf(target as HTMLButtonElement);
  if (at < 0) return;
  event.preventDefault();
  const next =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? buttons.length - 1
        : Math.min(buttons.length - 1, Math.max(0, at + (event.key === "ArrowDown" ? 1 : -1)));
  buttons.forEach((button, index) => {
    button.tabIndex = index === next ? 0 : -1;
  });
  buttons[next]?.focus();
}

/**
 * Unified diffs, file by file: a header with path, status and +/− counts that
 * folds the file away, hunk headers, old and new line numbers. With
 * `onAddComment`, every line gets a "+" (shown on hover or focus) that opens a
 * comment box under it; saved comments show under the line they anchor to.
 */
function DiffView({
  files,
  comments = NO_COMMENTS,
  onAddComment,
  onDeleteComment,
  empty = "No changes to show",
}: {
  files: DiffFile[];
  comments?: DiffComment[];
  /** Present when the person may comment; resolves once the comment is saved. */
  onAddComment?: (anchor: CommentAnchor, body: string) => Promise<void>;
  /** Removes a pending comment. */
  onDeleteComment?: (id: string) => Promise<void>;
  empty?: string;
}) {
  // Files whose fold state differs from their default: open for big ones, folded for the rest.
  const [toggled, setToggled] = useState<Set<string>>(new Set());

  const placed = useMemo(() => {
    const byPath = new Map<string, DiffComment[]>();
    for (const comment of comments) {
      const list = byPath.get(comment.path);
      if (list) list.push(comment);
      else byPath.set(comment.path, [comment]);
    }
    return files.map((file) => place(file, byPath.get(file.path) ?? NO_COMMENTS));
  }, [files, comments]);

  const keys = useMemo(() => files.map((file) => rowKeys(file.lines)), [files]);

  const toggle = useCallback((key: string) => {
    setToggled((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  if (files.length === 0) {
    return (
      <div className="empty diff-empty">
        <strong>{empty}</strong>
      </div>
    );
  }

  return (
    <div className="diff-view">
      {files.map((file, fileIndex) => {
        const key = fileKey(file);
        const big = file.lines.length > FOLD_LINES;
        const open = big ? toggled.has(key) : !toggled.has(key);
        const { byRow, outside } = placed[fileIndex];
        const firstCommentable = onAddComment ? file.lines.findIndex((line) => anchorOf(line) !== null) : -1;
        return (
          <section className="diff-file" key={key} aria-label={file.path}>
            <h3 className="diff-file-head">
              <button type="button" className="diff-fold" aria-expanded={open} onClick={() => toggle(key)}>
                <Icon name={open ? "chevdown" : "chevron"} size={12} />
                <span className="mono diff-path" title={file.path}>
                  {file.previousPath ? `${file.previousPath} → ${file.path}` : file.path}
                </span>
                {file.status !== "modified" ? <span className="chip">{STATUS_LABEL[file.status] ?? file.status}</span> : null}
                <span className="mono diff-stat">
                  <span className="diff-plus">+{file.additions}</span> <span className="diff-minus">−{file.deletions}</span>
                </span>
              </button>
            </h3>
            {open ? (
              file.lines.length === 0 ? (
                <p className="diff-note dim small">
                  {file.binary ? "Binary file, or too large to show here." : "No line changes (mode or name only)."}
                </p>
              ) : (
                <div className="diff-code" role="group" aria-label={`Changes in ${file.path}`} onKeyDown={moveBetweenLines}>
                  {file.lines.map((line, index) => (
                    <LineRow
                      key={keys[fileIndex][index]}
                      path={file.path}
                      line={line}
                      comments={byRow[index]}
                      tabbable={index === firstCommentable}
                      onAddComment={onAddComment}
                      onDeleteComment={onDeleteComment}
                    />
                  ))}
                  {outside.length > 0 ? (
                    <div className="diff-outside">
                      <span className="dim small">On lines outside this diff</span>
                      {outside.map((comment) => (
                        <SavedComment key={comment.id} comment={comment} onDelete={onDeleteComment} />
                      ))}
                    </div>
                  ) : null}
                </div>
              )
            ) : big ? (
              <button type="button" className="secondary diff-show" onClick={() => toggle(key)}>
                Show diff ({file.lines.length} lines)
              </button>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}

export default memo(DiffView);

const LineRow = memo(function LineRow({
  path,
  line,
  comments,
  tabbable,
  onAddComment,
  onDeleteComment,
}: {
  path: string;
  line: DiffLine;
  comments: DiffComment[];
  /** The one "+" of the file that Tab reaches; arrow keys reach the rest. */
  tabbable: boolean;
  onAddComment?: (anchor: CommentAnchor, body: string) => Promise<void>;
  onDeleteComment?: (id: string) => Promise<void>;
}) {
  const [composing, setComposing] = useState(false);
  const plusRef = useRef<HTMLButtonElement>(null);
  const anchor = anchorOf(line);
  const mark = line.kind === "add" ? "+" : line.kind === "del" ? "-" : line.kind === "context" ? " " : "";
  // A removed line and an added one can share a number; the side tells them apart.
  const label = anchor ? `Comment on ${anchor.side === "LEFT" ? "removed " : ""}line ${anchor.line} of ${path}` : "";

  return (
    <Fragment>
      <div className={`diff-line ${line.kind}`}>
        <span className="diff-cbtn-cell">
          {onAddComment && anchor ? (
            <button
              ref={plusRef}
              type="button"
              className="diff-cbtn"
              tabIndex={tabbable ? 0 : -1}
              aria-label={label}
              aria-expanded={composing}
              onClick={() => setComposing(true)}
            >
              +
            </button>
          ) : null}
        </span>
        <span className="diff-num">{line.oldLine ?? ""}</span>
        <span className="diff-num">{line.newLine ?? ""}</span>
        <span className="diff-text">{line.kind === "hunk" || line.kind === "note" ? line.text : `${mark}${line.text}`}</span>
      </div>
      {comments.map((comment) => (
        <SavedComment key={comment.id} comment={comment} onDelete={onDeleteComment} />
      ))}
      {composing && anchor && onAddComment ? (
        <CommentBox
          label={label}
          onCancel={() => {
            setComposing(false);
            plusRef.current?.focus();
          }}
          onSave={async (body) => {
            await onAddComment({ path, ...anchor }, body);
            setComposing(false);
          }}
        />
      ) : null}
    </Fragment>
  );
});

function CommentBox({
  label,
  onCancel,
  onSave,
}: {
  label: string;
  onCancel: () => void;
  onSave: (body: string) => Promise<void>;
}) {
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    box.current?.focus();
    box.current?.scrollIntoView?.({ block: "nearest" });
  }, []);

  async function save() {
    if (busy) return;
    if (!body.trim()) {
      box.current?.focus();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSave(body.trim());
    } catch (cause) {
      setError(String(cause instanceof Error ? cause.message : cause));
      setBusy(false);
    }
  }

  return (
    <div className="diff-cmt diff-compose">
      <textarea
        ref={box}
        aria-label={label}
        rows={2}
        value={body}
        placeholder="Leave a comment on this line"
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void save();
          }
        }}
      />
      {error ? <p className="error" role="alert">{error}</p> : null}
      <span className="od-row diff-cmt-actions">
        <button type="button" className="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
        <button type="button" className="primary" onClick={() => void save()} disabled={busy || !body.trim()}>
          Comment
        </button>
      </span>
    </div>
  );
}

function SavedComment({ comment, onDelete }: { comment: DiffComment; onDelete?: (id: string) => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <div className={comment.pending ? "diff-cmt pending" : "diff-cmt"}>
      <span className="od-row diff-cmt-head">
        <strong>{comment.authorLogin === "you" ? "You" : comment.authorLogin}</strong>
        <span className="dim small">{fmtWhen(comment.createdAt)}</span>
        {comment.onlyInHulahoop ? (
          <span className="chip warn" title="There is no GitHub token to send it with; add one in Settings → Personal">
            Only in Hulahoop
          </span>
        ) : comment.pending ? (
          <span className="chip" title="Sent to GitHub with your next review">Pending</span>
        ) : null}
        {comment.pending && onDelete ? (
          <button
            type="button"
            className="link danger-link diff-cmt-delete"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(null);
              onDelete(comment.id).catch((cause) => {
                setError(String(cause instanceof Error ? cause.message : cause));
                setBusy(false);
              });
            }}
          >
            Delete
          </button>
        ) : null}
      </span>
      <span className="diff-cmt-body">{comment.body}</span>
      {error ? <p className="error" role="alert">{error}</p> : null}
    </div>
  );
}
