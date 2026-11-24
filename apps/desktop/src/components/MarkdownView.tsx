import { useLayoutEffect, useMemo, useRef, type ReactNode } from "react";
import { paintMentions, renderMarkdown } from "./markdown";

/**
 * Markdown, rendered read-only through the same TipTap schema the editors use,
 * so a comment or a description looks the same shown as it did written. `@Name`
 * mentions are painted as chips; pass `mentions` to paint only known names.
 */
export default function MarkdownView({
  value,
  mentions,
  className,
  empty,
}: {
  /** Markdown in. */
  value: string;
  /** Names that count as mentions. Omit to paint every `@word`. */
  mentions?: string[];
  className?: string;
  /** Shown instead when the markdown is blank. */
  empty?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const html = useMemo(() => renderMarkdown(value), [value]);
  const mentionKey = mentions?.join("\u0000");

  useLayoutEffect(() => {
    const root = ref.current;
    if (!root) return;
    root.innerHTML = html;
    paintMentions(root, mentions);
    // Task checkboxes show their state; ticking them is the editor's job.
    root.querySelectorAll<HTMLInputElement>('input[type="checkbox"]').forEach((box) => {
      box.disabled = true;
    });
    root.querySelectorAll<HTMLAnchorElement>("a[href]").forEach((link) => {
      link.target = "_blank";
      link.rel = "noopener noreferrer";
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [html, mentionKey]);

  if (!value.trim() && empty) return <>{empty}</>;
  return <div ref={ref} className={`md richtext-prose ${className ?? ""}`.trim()} />;
}
