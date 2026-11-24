import type { SuggestionKeyDownProps, SuggestionProps } from "@tiptap/suggestion";
import { EditorContent, useEditor } from "@tiptap/react";
import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Avatar } from "./TaskBits";
import { editorMarkdown, isMentionableName, MarkdownMention, markdownExtensions } from "./markdown";

/** Someone a comment can `@`: an agent, or Lumi. */
export interface MentionCandidate {
  id: string;
  name: string;
  kind: "agent" | "lumi";
  /** The muted line beside the name; defaults to `Agent` or `Assistant`. */
  subtitle?: string;
}

type Pop = {
  items: MentionCandidate[];
  index: number;
  rect: DOMRect | null;
  pick: (item: MentionCandidate) => void;
};

/**
 * A markdown composer that knows the team. Typing `@` opens a picker at the
 * caret; the chosen name is stored as plain `@Name` text, which is what the
 * server reads to notify an agent or Lumi. A name the server could not read
 * back out of the text (a dot, brackets, more than three words) is not offered,
 * so every chip in the composer is one the server sees. ⌘↵ submits.
 */
export default function MentionEditor({
  value,
  onChange,
  candidates,
  onSubmit,
  placeholder = "Write a comment. Markdown works, and agents working on this task will see it.",
  ariaLabel = "Comment",
  minHeight = 72,
  autoFocus = false,
}: {
  /** Markdown in; set it to "" to clear after sending. */
  value: string;
  /** Markdown out, on every change. */
  onChange: (markdown: string) => void;
  candidates: MentionCandidate[];
  /** ⌘↵ (Ctrl↵ elsewhere). */
  onSubmit?: () => void;
  placeholder?: string;
  ariaLabel?: string;
  minHeight?: number;
  autoFocus?: boolean;
}) {
  const emitted = useRef(value);
  const candidatesRef = useRef(candidates);
  candidatesRef.current = candidates;
  const submitRef = useRef(onSubmit);
  submitRef.current = onSubmit;
  const [pop, setPop] = useState<Pop | null>(null);
  const popRef = useRef<Pop | null>(null);
  const show = (next: Pop | null) => {
    popRef.current = next;
    setPop(next);
  };

  const editor = useEditor({
    extensions: [
      ...markdownExtensions({ placeholder }),
      MarkdownMention.configure({
        HTMLAttributes: { class: "mention" },
        renderText: ({ node }) => `@${node.attrs.label ?? node.attrs.id}`,
        renderHTML: ({ node, options }) => ["span", options.HTMLAttributes, `@${node.attrs.label ?? node.attrs.id}`],
        suggestion: {
          char: "@",
          items: ({ query }: { query: string }) => {
            const needle = query.toLowerCase();
            return candidatesRef.current
              .filter((entry) => isMentionableName(entry.name) && entry.name.toLowerCase().includes(needle))
              .slice(0, 8);
          },
          command: ({ editor: live, range, props }) => {
            const item = props as unknown as MentionCandidate;
            live
              .chain()
              .focus()
              .insertContentAt(range, [
                { type: "mention", attrs: { id: item.id, label: item.name } },
                { type: "text", text: " " },
              ])
              .run();
          },
          render: () => {
            const open = (props: SuggestionProps<MentionCandidate>) =>
              show({
                items: props.items,
                index: Math.min(popRef.current?.index ?? 0, Math.max(props.items.length - 1, 0)),
                rect: props.clientRect?.() ?? null,
                pick: (item) => props.command(item as never),
              });
            return {
              onStart: (props: SuggestionProps<MentionCandidate>) => {
                popRef.current = null;
                open(props);
              },
              onUpdate: open,
              onExit: () => show(null),
              onKeyDown: ({ event }: SuggestionKeyDownProps) => {
                const current = popRef.current;
                if (!current) return false;
                if (event.key === "Escape") {
                  show(null);
                  return true;
                }
                if (!current.items.length) return false;
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  const step = event.key === "ArrowDown" ? 1 : -1;
                  show({ ...current, index: (current.index + step + current.items.length) % current.items.length });
                  return true;
                }
                if (event.key === "Enter" || event.key === "Tab") {
                  if (event.metaKey || event.ctrlKey) return false;
                  current.pick(current.items[current.index]);
                  return true;
                }
                return false;
              },
            };
          },
        },
      }),
    ],
    content: value,
    autofocus: autoFocus,
    editorProps: {
      attributes: {
        class: "richtext-prose",
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": ariaLabel,
        spellcheck: "true",
      },
      handleKeyDown: (_view, event) => {
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && submitRef.current) {
          event.preventDefault();
          submitRef.current();
          return true;
        }
        return false;
      },
    },
    onUpdate: ({ editor: live }) => {
      const markdown = editorMarkdown(live);
      emitted.current = markdown;
      onChange(markdown);
    },
  });

  // The editor keeps focus while the picker is open, so it names the listbox
  // and the highlighted option for a screen reader.
  const listId = useId();
  const open = Boolean(pop && pop.rect && pop.items.length);
  const active = open && pop ? `${listId}-${pop.index}` : null;
  useEffect(() => {
    const dom = editor?.view.dom;
    if (!dom) return;
    dom.setAttribute("aria-autocomplete", "list");
    if (open) dom.setAttribute("aria-controls", listId);
    else dom.removeAttribute("aria-controls");
    if (active) dom.setAttribute("aria-activedescendant", active);
    else dom.removeAttribute("aria-activedescendant");
  }, [editor, open, active, listId]);

  useEffect(() => {
    if (!editor) return;
    if (value === emitted.current) return;
    emitted.current = value;
    editor.commands.setContent(value, { emitUpdate: false });
  }, [editor, value]);

  return (
    <div className="richtext mention-editor" style={{ minHeight }}>
      <EditorContent editor={editor} />
      {open && pop && pop.rect
        ? createPortal(
            <div
              id={listId}
              className="mention-menu"
              role="listbox"
              aria-label="Mention"
              style={{ left: pop.rect.left, top: pop.rect.bottom + 4 }}
              onMouseDown={(event) => event.preventDefault()}
            >
              {pop.items.map((item, index) => (
                <button
                  type="button"
                  key={`${item.kind}:${item.id}`}
                  id={`${listId}-${index}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={index === pop.index}
                  className={index === pop.index ? "on" : undefined}
                  onMouseEnter={() => show({ ...pop, index })}
                  onClick={() => pop.pick(item)}
                >
                  <Avatar name={item.name} kind={item.kind} />
                  <span>{item.name}</span>
                  <small>{item.subtitle ?? (item.kind === "lumi" ? "Assistant" : "Agent")}</small>
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </div>
  );
}
