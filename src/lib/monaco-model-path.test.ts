import { describe, expect, it } from "vitest"
import { buildMonacoModelPath } from "@/lib/monaco-model-path"

describe("buildMonacoModelPath", () => {
  it("keys pathless tabs on the tab id", () => {
    expect(buildMonacoModelPath(null, "diff:working-all:1")).toBe(
      "inmemory://model/diff%3Aworking-all%3A1"
    )
  })

  it("maps absolute paths to file:/// without slash inflation", () => {
    expect(buildMonacoModelPath("/repo/src/a.ts", "x")).toBe(
      "file:///repo/src/a.ts"
    )
  })

  it("keeps UNC identity distinct from the single-slash form", () => {
    const unc = buildMonacoModelPath("//server/share/a.ts", "x")
    const posix = buildMonacoModelPath("/server/share/a.ts", "x")
    expect(unc).toBe("file://server/share/a.ts")
    expect(posix).toBe("file:///server/share/a.ts")
    expect(unc).not.toBe(posix)
  })

  it("encodes special characters per segment", () => {
    expect(buildMonacoModelPath("/repo/a b#c.ts", "x")).toBe(
      "file:///repo/a%20b%23c.ts"
    )
  })
})
