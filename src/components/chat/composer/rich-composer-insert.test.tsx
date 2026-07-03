import { act, render, waitFor } from "@testing-library/react"
import { Editor } from "@tiptap/core"
import { createRef } from "react"
import { describe, expect, it, vi } from "vitest"

import { buildComposerExtensions } from "./editor-config"
import { buildResilientMarkdownNodes } from "./markdown-insert"
import { RichComposer, type RichComposerHandle } from "./rich-composer"

async function mount() {
  const ref = createRef<RichComposerHandle>()
  render(<RichComposer ref={ref} />)
  await waitFor(() => expect(ref.current?.getEditor()).not.toBeNull(), {
    timeout: 5000,
  })
  const handle = ref.current
  const editor = handle?.getEditor()
  if (!handle || !editor) throw new Error("editor not mounted")
  return { handle, editor }
}

// A message mixing supported blocks (heading, list, code) with unsupported ones
// (GFM table, task-list checkboxes) — the shape that used to make the click
// dead. The "任务清单：" paragraph keeps marked from merging the plain list and
// the task list into one token.
const MIXED = [
  "# 多阶段任务",
  "",
  "普通段落 with **bold**",
  "",
  "| 状态项 | 内容 |",
  "| --- | --- |",
  "| 当前阶段 | 待启动 |",
  "",
  "普通列表：",
  "",
  "- 普通项 A",
  "- 普通项 B",
  "",
  "任务清单：",
  "",
  "- [ ] 文档已完整读取",
  "- [x] 已核实",
  "",
  "目标功能：<任务目标摘要>",
].join("\n")

describe("RichComposer.insertMarkdownAtCursor", () => {
  it("still parses supported Markdown into rich nodes", async () => {
    const { handle, editor } = await mount()
    act(() => handle.insertMarkdownAtCursor("**bold** text"))
    expect(JSON.stringify(editor.getJSON())).toContain('"type":"bold"')
    expect(editor.getText()).toContain("bold text")
  })

  it("keeps supported blocks rich and degrades only the unsupported ones", async () => {
    const { handle, editor } = await mount()
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})

    expect(() => {
      act(() => handle.insertMarkdownAtCursor(MIXED))
    }).not.toThrow()

    const json = JSON.stringify(editor.getJSON())
    // Supported blocks kept their rich structure…
    expect(json).toContain('"type":"heading"')
    expect(json).toContain('"type":"bold"')
    expect(json).toContain('"type":"bulletList"') // the plain list

    const text = editor.getText()
    // …and the unsupported blocks survive verbatim as literal source.
    expect(text).toContain("| 当前阶段 | 待启动 |")
    expect(text).toContain("- [ ] 文档已完整读取")
    expect(text).toContain("目标功能：<任务目标摘要>")

    // Recovery ran (a diagnostic warning was emitted) and the heading text
    // appears exactly once — the failed fast path inserted nothing.
    expect(warn).toHaveBeenCalled()
    expect(text.match(/多阶段任务/g)).toHaveLength(1)
    warn.mockRestore()
  })
})

describe("buildResilientMarkdownNodes", () => {
  let editor: Editor
  const make = () =>
    (editor = new Editor({ extensions: buildComposerExtensions() }))

  it("emits rich nodes for supported blocks and text paragraphs for unsupported", () => {
    make()
    const nodes = buildResilientMarkdownNodes(editor, MIXED)

    const types = nodes.map((n) => n.type)
    expect(types).toContain("heading")
    expect(types).toContain("bulletList")

    // Table + task list degrade to plain-text paragraphs carrying the source.
    const paraText = (frag: string) =>
      nodes.some(
        (n) =>
          n.type === "paragraph" && (n.content?.[0]?.text ?? "").includes(frag)
      )
    expect(paraText("| 当前阶段 | 待启动 |")).toBe(true)
    expect(paraText("- [ ] 文档已完整读取")).toBe(true)

    // Every returned node is schema-valid, so inserting them cannot throw.
    expect(() =>
      nodes.forEach((n) => editor.schema.nodeFromJSON(n).check())
    ).not.toThrow()
    editor.destroy()
  })

  it("trims trailing block newlines from degraded source", () => {
    make()
    const nodes = buildResilientMarkdownNodes(
      editor,
      "| a | b |\n| --- | --- |\n| 1 | 2 |\n"
    )
    const text = nodes[0]?.content?.[0]?.text ?? ""
    expect(text.endsWith("|")).toBe(true) // no trailing "\n"
    editor.destroy()
  })
})
