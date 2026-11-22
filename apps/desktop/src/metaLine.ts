import { Node } from "@tiptap/core";

/** The lines the parser reads: who handles a status, and where it goes next. */
const META = /^(Users|Next|Turns):/i;

function attr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * A metadata line inside the formatted document.
 *
 * `WORKFLOWS.md` is prose with a few machine-read lines in it. A plain markdown
 * editor folds `Users: human` into the paragraph that follows — CommonMark reads
 * a single newline as a soft break — and the file stops parsing. This node keeps
 * each such line a block of its own, with the text *editable in place*: markdown-it
 * hands every one back as its own token, the serializer writes the line exactly as
 * it stands, and the prose around it keeps behaving like prose.
 */
export const MetaLine = Node.create({
  name: "metaLine",
  group: "block",
  content: "inline*",
  // Deliberately not `isolating`: that flag makes the block a unit the cursor
  // skips, so arrow keys could never walk into or out of a metadata line and the
  // document would feel like a form. It behaves like any other block.

  parseHTML() {
    return [{ tag: "div[data-meta]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", { ...HTMLAttributes, class: "wf-meta-line" }, 0];
  },

  addStorage() {
    return {
      markdown: {
        serialize(
          state: {
            write: (text: string, escape?: boolean) => void;
            closeBlock: (node: unknown) => void;
          },
          node: { textContent: string },
        ) {
          // Verbatim: these lines are read by a parser, so no escaping may creep in.
          state.write(node.textContent, false);
          state.closeBlock(node);
        },
        parse: {
          setup(markdownit: {
            block: { ruler: { before: (name: string, rule: string, fn: unknown) => void } };
            renderer: { rules: Record<string, unknown> };
          }) {
            markdownit.block.ruler.before(
              "paragraph",
              "wf_meta",
              (
                state: {
                  bMarks: number[];
                  tShift: number[];
                  eMarks: number[];
                  src: string;
                  line: number;
                  push: (type: string, tag: string, nesting: number) => {
                    block: boolean;
                    content: string;
                    map: number[];
                    markup: string;
                  };
                },
                startLine: number,
                _endLine: number,
                silent: boolean,
              ) => {
                const start = state.bMarks[startLine] + state.tShift[startLine];
                const end = state.eMarks[startLine];
                const line = state.src.slice(start, end).trim();
                if (!META.test(line)) return false;
                if (silent) return true;
                const token = state.push("wf_meta", "div", 0);
                token.block = true;
                token.content = line;
                token.map = [startLine, startLine + 1];
                token.markup = "";
                state.line = startLine + 1;
                return true;
              },
            );
            markdownit.renderer.rules.wf_meta = (tokens: { content: string }[], index: number) =>
              `<div data-meta="line">${attr(tokens[index].content)}</div>`;
          },
        },
      },
    };
  },
});

export default MetaLine;
