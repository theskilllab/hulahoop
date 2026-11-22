import { Node } from "@tiptap/core";

/** The lines the parser reads: who handles a status, and where it goes next. */
const META = /^(Users|Next):/i;

function attr(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

/**
 * A metadata line inside a rich document.
 *
 * `WORKFLOWS.md` is prose with five machine-read lines in it. A plain markdown
 * editor folds `Users: human` into the paragraph after it — CommonMark treats a
 * single newline as a soft break — and the file stops parsing. This node keeps
 * those lines as atoms: markdown-it hands each one back as its own block, the
 * serializer writes it out verbatim, and the document can be edited as one piece
 * without the contract underneath it moving.
 */
export const MetaLine = Node.create({
  name: "metaLine",
  group: "block",
  atom: true,
  selectable: true,
  draggable: false,

  addAttributes() {
    return {
      text: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-meta") ?? element.textContent ?? "",
        renderHTML: () => ({}),
      },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-meta]" }];
  },

  renderHTML({ node }) {
    return [
      "div",
      { "data-meta": node.attrs.text, class: "wf-meta-line" },
      ["code", { class: "mono" }, node.attrs.text as string],
    ];
  },

  addStorage() {
    return {
      markdown: {
        serialize(state: { write: (text: string) => void; closeBlock: (node: unknown) => void }, node: { attrs: { text?: string } }) {
          state.write(node.attrs.text ?? "");
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
              (state: {
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
              }, startLine: number, _endLine: number, silent: boolean) => {
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
              `<div data-meta="${attr(tokens[index].content)}"></div>`;
          },
        },
      },
    };
  },
});

export default MetaLine;
