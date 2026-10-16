import { syntaxTree } from "@codemirror/language";
import type { Range } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";

/**
 * Live-preview decorations for Markdown.
 *
 * Everything stays plain text (that is the source of truth); this plugin only
 * styles what the syntax tree already knows:
 *  - headings, quotes, code blocks, rules get line styling;
 *  - bold/italic/strikethrough/code/link spans get inline styling;
 *  - the syntax marks (`#`, `**`, backticks, …) are dimmed, and hidden
 *    entirely on lines that do not contain the cursor.
 */
export function livePreview() {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = build(view);
      }

      update(update: ViewUpdate) {
        if (
          update.docChanged ||
          update.selectionSet ||
          update.viewportChanged ||
          update.focusChanged
        ) {
          this.decorations = build(update.view);
        }
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}

const liveTheme = EditorView.baseTheme({
  ".cm-md-mark": { color: "var(--fg-muted)", opacity: "0.55" },
  ".cm-md-h1": { fontSize: "1.7em", fontWeight: "700", lineHeight: "1.35" },
  ".cm-md-h2": { fontSize: "1.42em", fontWeight: "700", lineHeight: "1.4" },
  ".cm-md-h3": { fontSize: "1.2em", fontWeight: "650", lineHeight: "1.45" },
  ".cm-md-h4, .cm-md-h5, .cm-md-h6": { fontWeight: "650" },
  ".cm-md-h6": { opacity: "0.85" },
  ".cm-md-strong": { fontWeight: "700" },
  ".cm-md-em": { fontStyle: "italic" },
  ".cm-md-strike": { textDecoration: "line-through", opacity: "0.75" },
  ".cm-md-code": {
    fontFamily: "var(--font-mono)",
    background: "var(--code-bg)",
    borderRadius: "4px",
    padding: "0 3px",
  },
  ".cm-md-codeblock": { fontFamily: "var(--font-mono)", background: "var(--code-bg)" },
  ".cm-md-quote": { borderLeft: "3px solid var(--border)" },
  ".cm-md-link": { color: "var(--accent)" },
  ".cm-md-url": { color: "var(--fg-muted)" },
  ".cm-md-hr": { color: "var(--fg-muted)" },
});

export const livePreviewTheme = [liveTheme];

function headingLevel(name: string): string {
  return name.charAt(name.length - 1);
}

function build(view: EditorView): DecorationSet {
  const { state } = view;
  const doc = state.doc;
  const decos: Range<Decoration>[] = [];
  const lineDecos = new Set<string>();

  // Lines that contain any cursor/selection are "active": their syntax marks
  // stay visible (and editable) instead of being hidden.
  const activeLines = new Set<number>();
  for (const range of state.selection.ranges) {
    const first = doc.lineAt(range.from).number;
    const last = doc.lineAt(range.to).number;
    for (let line = first; line <= last; line++) activeLines.add(line);
  }

  const pushLine = (lineFrom: number, cls: string) => {
    const key = `${lineFrom}:${cls}`;
    if (lineDecos.has(key)) return;
    lineDecos.add(key);
    decos.push(Decoration.line({ class: cls }).range(lineFrom));
  };

  const markOrHide = (from: number, to: number, lineNumber: number) => {
    if (to <= from) return;
    if (activeLines.has(lineNumber)) {
      decos.push(Decoration.mark({ class: "cm-md-mark" }).range(from, to));
    } else {
      decos.push(Decoration.replace({}).range(from, to));
    }
  };

  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter(node) {
        const name = node.name;
        if (name.startsWith("ATXHeading")) {
          pushLine(doc.lineAt(node.from).from, `cm-md-h${headingLevel(name)}`);
          return;
        }
        switch (name) {
          case "HeaderMark": {
            const line = doc.lineAt(node.from);
            // also swallow the single space that follows `#`
            const end =
              node.to < line.to && doc.sliceString(node.to, node.to + 1) === " "
                ? node.to + 1
                : node.to;
            markOrHide(node.from, end, line.number);
            return;
          }
          case "Emphasis":
            decos.push(Decoration.mark({ class: "cm-md-em" }).range(node.from, node.to));
            return;
          case "StrongEmphasis":
            decos.push(Decoration.mark({ class: "cm-md-strong" }).range(node.from, node.to));
            return;
          case "Strikethrough":
            decos.push(Decoration.mark({ class: "cm-md-strike" }).range(node.from, node.to));
            return;
          case "EmphasisMark":
          case "StrikethroughMark":
            markOrHide(node.from, node.to, doc.lineAt(node.from).number);
            return;
          case "InlineCode":
            decos.push(Decoration.mark({ class: "cm-md-code" }).range(node.from, node.to));
            return;
          case "CodeMark": {
            if (node.node.parent?.name === "InlineCode") {
              markOrHide(node.from, node.to, doc.lineAt(node.from).number);
            } else {
              // fences of a code block stay visible, just dimmed
              decos.push(
                Decoration.mark({ class: "cm-md-mark" }).range(node.from, node.to),
              );
            }
            return;
          }
          case "FencedCode": {
            const first = doc.lineAt(node.from).number;
            const last = doc.lineAt(Math.max(node.from, node.to - 1)).number;
            for (let line = first; line <= last; line++) {
              pushLine(doc.line(line).from, "cm-md-codeblock");
            }
            return;
          }
          case "Blockquote": {
            const first = doc.lineAt(node.from).number;
            const last = doc.lineAt(Math.max(node.from, node.to - 1)).number;
            for (let line = first; line <= last; line++) {
              pushLine(doc.line(line).from, "cm-md-quote");
            }
            return;
          }
          case "QuoteMark":
          case "ListMark":
          case "LinkMark":
            decos.push(Decoration.mark({ class: "cm-md-mark" }).range(node.from, node.to));
            return;
          case "Link":
          case "Image":
            decos.push(Decoration.mark({ class: "cm-md-link" }).range(node.from, node.to));
            return;
          case "URL":
            decos.push(Decoration.mark({ class: "cm-md-url" }).range(node.from, node.to));
            return;
          case "HorizontalRule":
            pushLine(doc.lineAt(node.from).from, "cm-md-hr");
            return;
        }
      },
    });
  }

  return Decoration.set(decos, true);
}
