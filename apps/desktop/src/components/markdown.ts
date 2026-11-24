import { Editor, Node, type Extensions } from "@tiptap/core";
import Image from "@tiptap/extension-image";
import Mention from "@tiptap/extension-mention";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import Placeholder from "@tiptap/extension-placeholder";
import { TableKit } from "@tiptap/extension-table";
import StarterKit from "@tiptap/starter-kit";
import { Markdown } from "tiptap-markdown";

/**
 * The one TipTap setup every prose field in the app shares: markdown in and out,
 * headings, lists, task lists, code, links and tables. The editor, the read-only
 * viewer and the comment composer all start here, so a description written in
 * one place renders the same everywhere else.
 */
export function markdownExtensions(options: { placeholder?: string } = {}): Extensions {
  return [
    StarterKit.configure({ heading: { levels: [1, 2, 3] }, text: false }),
    MarkdownText,
    // Inline, as markdown writes it: `![alt](src)` sits inside a paragraph, and
    // without the node an edit dropped every image an agent put in a description.
    Image.configure({ inline: true }),
    TaskList,
    TaskItem.configure({ nested: true }),
    TableKit.configure({ table: { resizable: false } }),
    Placeholder.configure({ placeholder: options.placeholder ?? "" }),
    Markdown.configure({ html: false, tightLists: true, linkify: true, breaks: false }),
  ];
}

/**
 * How a run of plain text goes out as markdown. tiptap-markdown escapes `<` and
 * `>` as `&lt;`/`&gt;` so typed text can't turn into HTML, but with `html:false`
 * markdown-it never reads HTML in the first place, and the entities were what got
 * stored: `Vec<String>` came back as `Vec&lt;String&gt;` for every agent reading
 * the raw body, and literally so inside a code span, where entities are not
 * decoded. So only an `html:true` setup escapes.
 *
 * Round trip, `html:false`: `a<b`, `x > y` and `` `Vec<T>` `` come out unchanged
 * (markdown's own escaping still runs, and puts `\>` before a `>` that starts a
 * line, so it stays text and does not become a quote). Parsed again, markdown-it
 * treats `<` and `>` as text, so the same characters come back.
 */
export function textToMarkdown(text: string, html: boolean): string {
  return html ? text.replace(/</g, "&lt;").replace(/>/g, "&gt;") : text;
}

/** StarterKit's text node, with the serializer above in place of tiptap-markdown's. */
const MarkdownText = Node.create({
  name: "text",
  group: "inline",
  addStorage() {
    return {
      markdown: {
        serialize(
          this: { editor: { storage: { markdown?: { options?: { html?: boolean } } } } },
          state: { text: (text: string) => void },
          node: { text?: string },
        ) {
          state.text(textToMarkdown(node.text ?? "", this.editor.storage.markdown?.options?.html === true));
        },
        parse: {},
      },
    };
  },
});

/** The mention node, written back to markdown as plain `@Name`: the server reads
 *  mentions out of the text, so the stored comment needs no special syntax. */
export const MarkdownMention = Mention.extend({
  addStorage() {
    return {
      markdown: {
        serialize(state: { write: (text: string) => void }, node: { attrs: { id: string | null; label?: string | null } }) {
          state.write(`@${node.attrs.label ?? node.attrs.id ?? ""}`);
        },
        parse: {},
      },
    };
  },
});

let renderer: Editor | null = null;
const rendered = new Map<string, string>();

/**
 * Markdown to HTML through the shared schema, for read-only views. One headless
 * editor does the parsing and the result is cached, so a long comment thread or
 * transcript costs one string per entry instead of one editor each. The HTML
 * comes out of the schema's own serializer with raw HTML disabled, so it only
 * ever holds the nodes and marks defined above.
 */
export function renderMarkdown(markdown: string): string {
  const hit = rendered.get(markdown);
  if (hit !== undefined) return hit;
  renderer ??= new Editor({ extensions: markdownExtensions(), editable: false });
  renderer.commands.setContent(markdown, { emitUpdate: false });
  const html = renderer.getHTML();
  if (rendered.size > 800) rendered.delete(rendered.keys().next().value as string);
  rendered.set(markdown, html);
  return html;
}

/**
 * The markdown an editor on this schema would store for `markdown`: what an
 * untouched description comes back as after a round trip. Two bodies with the
 * same normal form say the same thing, so saving one over the other is a no-op.
 */
export function normalizeMarkdown(markdown: string): string {
  renderer ??= new Editor({ extensions: markdownExtensions(), editable: false });
  renderer.commands.setContent(markdown, { emitUpdate: false });
  return editorMarkdown(renderer).trim();
}

// Mentions, read the way the server reads them (crates/server/src/mentions.rs):
// after an `@` that does not follow a letter or digit, up to three words of
// letters, digits, `_` and `-`, joined by single spaces; each prefix of those
// words is a candidate, longest first, and names compare without spaces or case.
const NAME_WORDS = 3;
const NAME_CHAR = /^[\p{Alphabetic}\p{N}_-]$/u;
const ALNUM = /^[\p{Alphabetic}\p{N}]$/u;

/** The code point at `index`, as a string (one or two UTF-16 units). */
function charAt(value: string, index: number): string {
  const point = value.codePointAt(index);
  return point === undefined ? "" : String.fromCodePoint(point);
}

/** The code point that ends just before `index`. */
function charBefore(value: string, index: number): string {
  if (index <= 0) return "";
  const low = value.charCodeAt(index - 1);
  return low >= 0xdc00 && low <= 0xdfff && index >= 2 ? value.slice(index - 2, index) : value.charAt(index - 1);
}

/** How the server compares names: no whitespace, lower case. */
export function normalizeMentionName(name: string): string {
  return name.replace(/\s+/gu, "").toLowerCase();
}

/** One `@` in some text and the names it could start, longest first. */
export interface MentionSpan {
  at: number;
  /** `end` is the index just past the candidate's last character. */
  candidates: { text: string; end: number }[];
}

/** Every `@` in `value` that the server would read a name from. */
export function mentionSpans(value: string): MentionSpan[] {
  const spans: MentionSpan[] = [];
  for (let at = value.indexOf("@"); at >= 0; at = value.indexOf("@", at + 1)) {
    if (ALNUM.test(charBefore(value, at))) continue;
    const ends: number[] = [];
    let index = at + 1;
    while (ends.length < NAME_WORDS) {
      const start = index;
      for (let char = charAt(value, index); char && NAME_CHAR.test(char); char = charAt(value, index)) index += char.length;
      if (index === start) break;
      ends.push(index);
      // Only a single space continues a name; punctuation ends it.
      if (value.charAt(index) === " ") index += 1;
      else break;
    }
    if (!ends.length) continue;
    spans.push({
      at,
      candidates: ends
        .map((end) => ({ text: value.slice(at + 1, end), end }))
        .reverse(),
    });
  }
  return spans;
}

/**
 * Whether `@name` written into a comment reaches the server as a mention. A name
 * with a dot, brackets, double spaces or more than three words does not, so the
 * picker leaves it out instead of inserting a chip nobody gets.
 */
export function isMentionableName(name: string): boolean {
  const wanted = normalizeMentionName(name);
  if (!wanted) return false;
  const span = mentionSpans(`@${name}`)[0];
  return Boolean(span?.candidates.some((candidate) => normalizeMentionName(candidate.text) === wanted));
}

/**
 * Wraps `@Name` in rendered markdown in a `.mention` chip. Only names in `names`
 * are painted when the list is given, matched like the server matches them, so
 * an email address or a stray `@` stays plain text; without the list the first
 * word after the `@` is painted. Code is left alone.
 */
export function paintMentions(root: HTMLElement, names?: string[]) {
  const known = names ? new Set(names.map(normalizeMentionName)) : null;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const texts: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const parent = node.parentElement;
    if (parent?.closest("code, pre, .mention")) continue;
    if (node.nodeValue?.includes("@")) texts.push(node as Text);
  }
  for (const text of texts) {
    const value = text.nodeValue ?? "";
    const parts: (string | HTMLElement)[] = [];
    let last = 0;
    for (const span of mentionSpans(value)) {
      if (span.at < last) continue;
      const hit = known
        ? span.candidates.find((candidate) => known.has(normalizeMentionName(candidate.text)))
        : span.candidates[span.candidates.length - 1];
      if (!hit) continue;
      parts.push(value.slice(last, span.at));
      const chip = document.createElement("span");
      chip.className = "mention";
      chip.textContent = `@${hit.text}`;
      parts.push(chip);
      last = hit.end;
    }
    if (!parts.length) continue;
    parts.push(value.slice(last));
    text.replaceWith(...parts);
  }
}

/** Read the markdown back out of an editor built on `markdownExtensions`. */
export function editorMarkdown(editor: { storage: unknown }): string {
  const storage = editor.storage as { markdown: { getMarkdown: () => string } };
  return storage.markdown.getMarkdown();
}
