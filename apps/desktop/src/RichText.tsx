import Placeholder from "@tiptap/extension-placeholder";
import { EditorContent, useEditor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { useEffect, useRef } from "react";
import { Markdown } from "tiptap-markdown";

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
}: {
  /** Markdown in. */
  value: string;
  /** Markdown out, on every change. */
  onChange: (markdown: string) => void;
  placeholder?: string;
  minHeight?: number;
  ariaLabel?: string;
  autoFocus?: boolean;
}) {
  // The last markdown this component emitted, so an echo of our own change does
  // not reset the document (and the caret) while a person is typing.
  const emitted = useRef(value);

  const editor = useEditor({
    extensions: [
      StarterKit.configure({ heading: { levels: [1, 2, 3] } }),
      Placeholder.configure({ placeholder: placeholder ?? "" }),
      Markdown.configure({ html: false, tightLists: true, linkify: true, breaks: false }),
    ],
    content: value,
    autofocus: autoFocus,
    editorProps: {
      attributes: {
        class: "richtext-prose",
        role: "textbox",
        "aria-multiline": "true",
        "aria-label": ariaLabel ?? "Rich text",
        spellcheck: "true",
      },
    },
    onUpdate: ({ editor: live }) => {
      const storage = live.storage as unknown as { markdown: { getMarkdown: () => string } };
      const markdown = storage.markdown.getMarkdown();
      emitted.current = markdown;
      onChange(markdown);
    },
  });

  // A value from outside (a reload, another record) replaces the document; a
  // value that is just our own last emission is left alone.
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
