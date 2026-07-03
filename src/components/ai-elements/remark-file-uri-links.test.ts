import { describe, expect, it } from "vitest"
import { remarkRewriteFileUriLinks } from "./remark-file-uri-links"

// Minimal mdast node shapes for the transform.
type Node = {
  type: string
  url?: string
  identifier?: string
  children?: Node[]
}

function linkTree(url: string): Node {
  return {
    type: "root",
    children: [
      {
        type: "paragraph",
        children: [{ type: "link", url, children: [{ type: "text" }] }],
      },
    ],
  }
}

function firstLinkUrl(tree: Node): string | undefined {
  let found: string | undefined
  const walk = (n: Node) => {
    if (n.type === "link") found = n.url
    n.children?.forEach(walk)
  }
  walk(tree)
  return found
}

function rewrite(url: string): string | undefined {
  const tree = linkTree(url)
  remarkRewriteFileUriLinks()(tree)
  return firstLinkUrl(tree)
}

describe("remarkRewriteFileUriLinks", () => {
  it("rewrites a POSIX file:// URI to a bare local path", () => {
    expect(rewrite("file:///Users/a/b.ts")).toBe("/Users/a/b.ts")
  })

  it("strips the leading slash before a Windows drive letter", () => {
    expect(rewrite("file:///C:/x/y.ts")).toBe("C:/x/y.ts")
  })

  it("emits a UNC file:// URI as a backslash UNC path (unambiguously local)", () => {
    // //server/share would be indistinguishable from a protocol-relative
    // web url downstream; the backslash form tags it as a local file.
    expect(rewrite("file://server/share/doc.md")).toBe(
      "\\\\server\\share\\doc.md"
    )
  })

  it("preserves fragments on rewritten links", () => {
    expect(rewrite("file:///Users/a/b.ts#L12")).toBe("/Users/a/b.ts#L12")
  })

  it("leaves non-file URLs untouched", () => {
    expect(rewrite("https://example.com/x")).toBe("https://example.com/x")
  })
})
