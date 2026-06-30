import { describe, expect, it } from "vitest"

import { resolveDefaultAgent } from "./resolve-default-agent"

describe("resolveDefaultAgent", () => {
  it("keeps folder default and inherit ahead of recent agent", () => {
    expect(
      resolveDefaultAgent({
        folderDefault: "claude_code",
        inherit: "gemini",
        recentAgent: "codex",
        sortedTypes: ["open_code"],
        fresh: true,
      })
    ).toEqual({
      agentType: "claude_code",
      provisional: false,
    })

    expect(
      resolveDefaultAgent({
        folderDefault: null,
        inherit: "gemini",
        recentAgent: "codex",
        sortedTypes: ["open_code"],
        fresh: true,
      })
    ).toEqual({
      agentType: "gemini",
      provisional: false,
    })
  })

  it("falls back to recent agent before sorted list", () => {
    expect(
      resolveDefaultAgent({
        folderDefault: null,
        inherit: null,
        recentAgent: "gemini",
        sortedTypes: ["codex", "gemini", "claude_code"],
        fresh: true,
      })
    ).toEqual({
      agentType: "gemini",
      provisional: false,
    })
  })

  it("still falls back to codex when nothing else exists", () => {
    expect(
      resolveDefaultAgent({
        folderDefault: null,
        inherit: null,
        recentAgent: null,
        sortedTypes: [],
        fresh: true,
      })
    ).toEqual({
      agentType: "codex",
      provisional: false,
    })
  })
})
