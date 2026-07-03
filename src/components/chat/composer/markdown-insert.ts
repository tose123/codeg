import type { Editor, JSONContent } from "@tiptap/core"

/** The subset of a `marked` block token we inspect (structurally typed to avoid
 *  a direct `marked` dependency). A `list` token carries per-item `task` flags. */
interface MarkedBlockToken {
  type: string
  raw: string
  items?: Array<{ task?: boolean }>
}

/** True for a `list` token with any task-list item (`- [ ]` / `- [x]`). */
function isTaskList(token: MarkedBlockToken): boolean {
  return (
    token.type === "list" &&
    Array.isArray(token.items) &&
    token.items.some((item) => item.task === true)
  )
}

/**
 * Recovery path for {@link "./rich-composer".RichComposerHandle.insertMarkdownAtCursor}.
 *
 * The composer's schema ({@link "./editor-config".buildComposerExtensions}) is
 * StarterKit-based and has no node for a GFM **table** or a **task-list**
 * checkbox (`- [ ]`). When authored Markdown containing one is inserted,
 * `@tiptap/markdown` parses it into nodes ProseMirror rejects and
 * `insertContent` throws a `RangeError` (it runs an unconditional
 * `node.check()`; `setContent`, by contrast, silently drops invalid nodes).
 *
 * A blanket "insert the whole thing as plain text" fallback would throw away the
 * formatting of every *supported* block too. Instead this splits the Markdown
 * into its top-level blocks (via the shared `marked` lexer, whose tokens carry
 * the exact `.raw` source) and keeps each block that the live schema *can*
 * represent as rich Markdown, degrading only the blocks it can't to their
 * literal source as a plain-text paragraph. So a message mixing a heading, a
 * list and a table renders the heading and list richly and shows the table as
 * its `| … |` source — which for a prompt template is the faithful result, and
 * round-trips unchanged to the agent.
 *
 * The returned nodes are all schema-valid, so inserting them cannot throw.
 */
export function buildResilientMarkdownNodes(
  editor: Editor,
  markdown: string
): JSONContent[] {
  const md = editor.markdown
  // No marked instance (older/misconfigured build): keep it all as one block.
  if (!md || !md.hasMarked()) return [sourceParagraph(markdown)]

  let tokens: MarkedBlockToken[]
  try {
    tokens = md.instance.lexer(markdown) as MarkedBlockToken[]
  } catch {
    return [sourceParagraph(markdown)]
  }

  const nodes: JSONContent[] = []
  for (const token of tokens) {
    // `space` tokens are the blank lines between blocks; separation is already
    // implied by emitting each block as its own node.
    if (!token.raw || token.type === "space") continue
    // A list with any task item (`- [ ]` / `- [x]`) is forced to degrade to
    // literal source: the composer schema has no task node, and while a *tight*
    // task list parses into invalid nodes (and would degrade anyway), a *loose*
    // one parses into a valid plain list that silently DROPS the checkbox
    // marker — data loss. Degrading preserves `[ ]`/`[x]` verbatim.
    const parsed = isTaskList(token)
      ? null
      : parseBlockIfSupported(editor, md, token.raw)
    if (parsed && parsed.length > 0) nodes.push(...parsed)
    else nodes.push(sourceParagraph(token.raw))
  }

  // Never return nothing (e.g. whitespace-only input): fall back to one block.
  return nodes.length > 0 ? nodes : [sourceParagraph(markdown)]
}

/**
 * Parse a single block's raw Markdown and return its Tiptap block nodes, but
 * only if every one is representable by the live schema. Returns `null` when the
 * block uses an unknown node type or produces schema-invalid content (i.e. what
 * would have thrown at insert time), so the caller can substitute literal text.
 */
function parseBlockIfSupported(
  editor: Editor,
  md: NonNullable<Editor["markdown"]>,
  raw: string
): JSONContent[] | null {
  try {
    const doc = md.parse(raw)
    const blocks = Array.isArray(doc.content) ? doc.content : []
    // `nodeFromJSON` throws on an unknown node type; `check()` throws on invalid
    // content — the same validation `insertContent` runs, done here per block.
    for (const block of blocks) editor.schema.nodeFromJSON(block).check()
    return blocks
  } catch {
    return null
  }
}

/**
 * A plain-text paragraph carrying literal Markdown source. Internal newlines
 * render as line breaks (the composer surface is `white-space: pre-wrap`) and
 * survive serialization, so the source round-trips. Trailing block newlines
 * (which `marked` folds into a token's `raw`) are trimmed.
 */
function sourceParagraph(raw: string): JSONContent {
  const text = raw.replace(/\n+$/, "")
  return {
    type: "paragraph",
    content: text ? [{ type: "text", text }] : [],
  }
}
