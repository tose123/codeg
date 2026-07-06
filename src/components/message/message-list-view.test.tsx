import { describe, expect, it } from "vitest"

import { singletonSourceTurns } from "./message-list-view"
import type { MessageTurn } from "@/lib/types"

function turn(id: string): MessageTurn {
  return { id, role: "assistant", blocks: [], timestamp: "" }
}

describe("singletonSourceTurns", () => {
  it("returns the same array reference for the same turn", () => {
    const t = turn("t1")
    const first = singletonSourceTurns(t)
    const second = singletonSourceTurns(t)
    // Reference stability is the whole point: it lets HistoricalMessageGroup's
    // memo bail out when an unchanged historical turn re-renders per token.
    expect(first).toBe(second)
    expect(first).toEqual([t])
  })

  it("returns distinct arrays for distinct turns", () => {
    const a = singletonSourceTurns(turn("a"))
    const b = singletonSourceTurns(turn("b"))
    expect(a).not.toBe(b)
  })
})
