import type { Extensions } from "@tiptap/core";
import { EditorContent, useEditor } from "@tiptap/react";
import { useEffect, useRef } from "react";
import { editorMarkdown, markdownExtensions } from "./components/markdown";

/**
 * A markdown editor that shows the formatted text while typing.
 *
 * Built on TipTap, which is the piece worth reusing: the value in and out is
 * markdown, so every prose field in the app — an issue's description, a comment,
 * an agent's AGENTS.md, a skill's body, an artifact's notes — can share one
 * component and store what it already stores.
 *
 * There is deliberately no toolbar. Markdown is the format, so `# `, `- `, `**`
 * and backticks do the work while the result looks like the page, and footers
 * only appear where a screen needs a counter or a hint.
 */
export default function RichText({
  value,
  onChange,
  placeholder,
  minHeight = 120,
  ariaLabel,
  autoFocus = false,
  editable = true,
  extensions = [],
  onSubmit,
}: {
  /** Markdown in. */
  value: string;
  /** Markdown out, on every change. */
  onChange: (markdown: string) => void;
  placeholder?: string;
  minHeight?: number;
  ariaLabel?: string;
  autoFocus?: boolean;
  /** Read-only renders the same formatted text: viewing a document is not a
   *  different screen from writing it, only the same one with the caret off. */
  editable?: boolean;
  /** Extra nodes this field needs, e.g. the workflow's metadata lines. */
  extensions?: Extensions;
  /** ⌘↵ (Ctrl↵ elsewhere); `shift` when ⇧ is held too. Without it ⌘↵ is a line break. */
  onSubmit?: (shift: boolean) => void;
}) {
  const submitRef = useRef(onSubmit);
  submitRef.current = onSubmit;
  // The last markdown this component emitted, so an echo of our own change does
  // not reset the document (and the caret) while a person is typing.
  const emitted = useRef(value);

  const editor = useEditor({
    extensions: [...markdownExtensions({ placeholder }), ...extensions],
    content: value,
    autofocus: autoFocus,
    editable,
    editorProps: {
      attributes: {
        class: "richtext-prose",
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": ariaLabel ?? "Rich text",
        spellcheck: "true",
      },
      handleKeyDown: (_view, event) => {
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && submitRef.current) {
          event.preventDefault();
          submitRef.current(event.shiftKey);
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

  // A value from outside (a reload, another record) replaces the document; a
  // value that is just our own last emission is left alone.
  // `useEditor` reads `editable` once, so a card that opens for editing has to
  // flip it here.
  useEffect(() => {
    editor?.setEditable(editable);
  }, [editor, editable]);

  useEffect(() => {
    if (!editor) return;
    if (value === emitted.current) return;
    emitted.current = value;
    editor.commands.setContent(value, { emitUpdate: false });
  }, [editor, value]);

  return (
    <div className="richtext" style={{ minHeight }}>
      <EditorContent editor={editor} />
    </div>
  );
}
