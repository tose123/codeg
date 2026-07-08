/**
 * Background-overlay slice: out-of-turn transcript turns pushed by the backend
 * watcher (`background_activity` events) into the conversation runtime store.
 *
 * Covered invariants:
 *  - upsert semantics keyed by turn id (append new, replace-in-place a
 *    still-growing turn, adopt the event watermark), materializing the
 *    session when the conversation isn't loaded yet;
 *  - the watermark hand-off: a detail (re)fetch retires exactly the overlay
 *    entries its `transcript_watermark` covers — never more (silent loss),
 *    never fewer than none (duplicates linger only until covered);
 *  - timeline assembly: overlay turns render as persisted-phase entries after
 *    `detail.turns`, interleaved with `localTurns` by timestamp so a
 *    foreground exchange completed BETWEEN background turns keeps wall order;
 *  - a background-only session still cold-fetches detail (the overlay must
 *    not satisfy the "has active data" fetch skip).
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  BACKGROUND_OVERLAY_HARD_CAP,
  resetConversationRuntimeStore,
  selectTimelineTurns,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
}))

const { getFolderConversation } = await import("@/lib/api")
const mockGetFolderConversation = vi.mocked(getFolderConversation)

function turn(
  id: string,
  text: string,
  timestamp = "2026-07-07T03:47:08.000Z"
): MessageTurn {
  return {
    id,
    role: "assistant",
    blocks: [{ type: "text", text }],
    timestamp,
  }
}

function detail(
  overrides: Partial<DbConversationDetail> = {}
): DbConversationDetail {
  return {
    summary: {
      id: 7,
      folder_id: 1,
      agent_type: "claude_code",
      title: "t",
      title_locked: false,
      status: "in_progress",
      kind: "regular",
      model: null,
      git_branch: null,
      external_id: "sess-7",
      message_count: 0,
      child_count: 0,
      created_at: "2026-07-07T03:40:00.000Z",
      updated_at: "2026-07-07T03:40:00.000Z",
      pinned_at: null,
    },
    turns: [],
    session_stats: null,
    ...overrides,
  }
}

function actions() {
  return useConversationRuntimeStore.getState().actions
}

function session(conversationId: number) {
  return useConversationRuntimeStore
    .getState()
    .byConversationId.get(conversationId)
}

async function flushMicrotasks() {
  await Promise.resolve()
  await Promise.resolve()
}

beforeEach(() => {
  resetConversationRuntimeStore()
  mockGetFolderConversation.mockReset()
})

afterEach(() => {
  resetConversationRuntimeStore()
})

describe("APPLY_BACKGROUND_ACTIVITY", () => {
  it("materializes the session and upserts by turn id", () => {
    actions().applyBackgroundActivity(7, [turn("bg-100-0", "step one")], 100)
    expect(session(7)?.backgroundTurns).toHaveLength(1)
    expect(session(7)?.backgroundTurns[0].watermark).toBe(100)

    // A still-growing turn re-emits under the SAME id: replaced in place,
    // watermark adopted; a new id appends after it.
    actions().applyBackgroundActivity(
      7,
      [turn("bg-100-0", "step one + more"), turn("bg-100-1", "step two")],
      220
    )
    const entries = session(7)!.backgroundTurns
    expect(entries.map((e) => e.turn.id)).toEqual(["bg-100-0", "bg-100-1"])
    expect(entries[0].turn.blocks[0]).toMatchObject({
      text: "step one + more",
    })
    expect(entries.map((e) => e.watermark)).toEqual([220, 220])
  })

  it("drops the oldest entries past the hard cap (degraded-retirement backstop)", () => {
    // Simulate retirement never running (refetch failing) while autonomous
    // turns keep arriving: the overlay must stay bounded, oldest-first.
    for (let i = 0; i < BACKGROUND_OVERLAY_HARD_CAP + 5; i++) {
      actions().applyBackgroundActivity(7, [turn(`bg-x-${i}`, `t${i}`)], i)
    }
    const entries = session(7)!.backgroundTurns
    expect(entries).toHaveLength(BACKGROUND_OVERLAY_HARD_CAP)
    expect(entries[0].turn.id).toBe("bg-x-5")
    expect(entries[entries.length - 1].turn.id).toBe(
      `bg-x-${BACKGROUND_OVERLAY_HARD_CAP + 4}`
    )
  })
})

describe("watermark hand-off on FETCH_DETAIL_SUCCESS", () => {
  it("retires exactly the entries the detail's watermark covers", async () => {
    actions().applyBackgroundActivity(7, [turn("bg-1-0", "old")], 100)
    actions().applyBackgroundActivity(7, [turn("bg-1-1", "new")], 300)

    // Refetch whose parse consumed 200 bytes: covers the 100-watermark entry
    // (its content is in `detail.turns` now), NOT the 300 one.
    mockGetFolderConversation.mockResolvedValueOnce(
      detail({ transcript_watermark: 200, turns: [turn("turn-0", "old")] })
    )
    actions().refetchDetail(7)
    await flushMicrotasks()

    const entries = session(7)!.backgroundTurns
    expect(entries.map((e) => e.turn.id)).toEqual(["bg-1-1"])
  })

  it("keeps every entry when the detail carries no watermark", async () => {
    actions().applyBackgroundActivity(7, [turn("bg-1-0", "x")], 100)
    mockGetFolderConversation.mockResolvedValueOnce(detail())
    actions().refetchDetail(7)
    await flushMicrotasks()
    expect(session(7)!.backgroundTurns).toHaveLength(1)
  })

  it("preserves array identity when nothing retires", async () => {
    actions().applyBackgroundActivity(7, [turn("bg-1-0", "x")], 500)
    const before = session(7)!.backgroundTurns
    mockGetFolderConversation.mockResolvedValueOnce(
      detail({ transcript_watermark: 200 })
    )
    actions().refetchDetail(7)
    await flushMicrotasks()
    expect(session(7)!.backgroundTurns).toBe(before)
  })
})

describe("timeline assembly", () => {
  it("renders overlay turns as persisted-phase entries after detail turns", async () => {
    mockGetFolderConversation.mockResolvedValueOnce(
      detail({
        transcript_watermark: 50,
        turns: [turn("turn-0", "history", "2026-07-07T03:40:05.000Z")],
      })
    )
    actions().fetchDetail(7)
    await flushMicrotasks()
    actions().applyBackgroundActivity(7, [turn("bg-60-0", "bg reply")], 120)

    const timeline = selectTimelineTurns(
      useConversationRuntimeStore.getState(),
      7
    )
    expect(timeline.map((t) => t.turn.id)).toEqual(["turn-0", "bg-60-0"])
    expect(timeline[1].phase).toBe("persisted")
    expect(new Set(timeline.map((t) => t.key)).size).toBe(timeline.length)
  })

  it("interleaves local and background turns by timestamp", () => {
    // Background turn at T1, foreground reply promoted to localTurns at T2,
    // background turn at T3 — wall order must hold in the timeline.
    actions().applyBackgroundActivity(
      7,
      [turn("bg-0-0", "bg early", "2026-07-07T03:41:00.000Z")],
      100
    )
    actions().appendOptimisticTurn(
      7,
      {
        id: "local-user",
        role: "user",
        blocks: [{ type: "text", text: "hi" }],
        timestamp: "2026-07-07T03:42:00.000Z",
      },
      "token-1"
    )
    actions().completeTurn(7, {
      id: "live-1",
      role: "assistant",
      content: [{ type: "text", text: "fg reply" }],
      startedAt: Date.parse("2026-07-07T03:42:30.000Z"),
    })
    actions().applyBackgroundActivity(
      7,
      [turn("bg-0-1", "bg late", "2026-07-07T03:43:00.000Z")],
      200
    )

    const timeline = selectTimelineTurns(
      useConversationRuntimeStore.getState(),
      7
    )
    const ids = timeline.map((t) => t.turn.id)
    expect(ids.indexOf("bg-0-0")).toBeLessThan(ids.indexOf("local-user"))
    expect(ids.indexOf("local-user")).toBeLessThan(ids.indexOf("bg-0-1"))
  })
})

describe("cold-fetch guard", () => {
  it("a background-only session still fetches detail", () => {
    actions().applyBackgroundActivity(7, [turn("bg-1-0", "x")], 100)
    mockGetFolderConversation.mockResolvedValueOnce(detail())
    actions().fetchDetail(7)
    expect(mockGetFolderConversation).toHaveBeenCalledTimes(1)
  })
})

describe("refetchDetail DB-id resolution", () => {
  // Regression: a conversation started as a new-chat draft keeps a virtual
  // (negative) runtime key forever; the DB row created on first send has a
  // different id. The settle-driven refetch dispatches on the runtime key —
  // it must FETCH with the bound DB id, or the backend errors on the virtual
  // id and the stale local turn (async sub-agent card frozen on its launch
  // ack) never flips to the persisted terminal state.
  it("fetches with the bound DB id and replaces stale local turns under the runtime key", async () => {
    const VIRTUAL = -7
    actions().setDbConversationId(VIRTUAL, 42)

    // Foreground turn completed live: the launch card's raw wire ack sits in
    // localTurns, exactly as after COMPLETE_TURN in production.
    actions().appendOptimisticTurn(
      VIRTUAL,
      {
        id: "u-1",
        role: "user",
        blocks: [{ type: "text", text: "run build in background" }],
        timestamp: "2026-07-07T08:38:53.000Z",
      },
      "token-1"
    )
    actions().completeTurn(VIRTUAL, {
      id: "live-1",
      role: "assistant",
      content: [{ type: "text", text: "Async agent launched successfully." }],
      startedAt: Date.parse("2026-07-07T08:39:06.000Z"),
    })
    expect(session(VIRTUAL)!.localTurns.length).toBeGreaterThan(0)

    // Settled refetch: the parser has folded the task-notification into the
    // launching turn (terminal marker) by now.
    mockGetFolderConversation.mockResolvedValueOnce(
      detail({
        summary: { ...detail().summary, id: 42 },
        transcript_watermark: 35582,
        turns: [turn("turn-0", "[[codeg-background-task]] terminal state")],
      })
    )
    actions().refetchDetail(VIRTUAL, { preserveLive: false })
    await flushMicrotasks()

    expect(mockGetFolderConversation).toHaveBeenCalledTimes(1)
    expect(mockGetFolderConversation).toHaveBeenCalledWith(42)
    // Result lands under the runtime key; the stale live buffers are gone and
    // the persisted (terminal) copy is what the timeline renders.
    expect(session(VIRTUAL)?.detail?.turns.map((t) => t.id)).toEqual(["turn-0"])
    expect(session(VIRTUAL)?.localTurns).toEqual([])
    const timeline = selectTimelineTurns(
      useConversationRuntimeStore.getState(),
      VIRTUAL
    )
    expect(timeline.map((t) => t.turn.id)).toEqual(["turn-0"])
  })

  it("falls back to the session key when no DB id is bound", async () => {
    mockGetFolderConversation.mockResolvedValueOnce(detail())
    actions().refetchDetail(7)
    await flushMicrotasks()
    expect(mockGetFolderConversation).toHaveBeenCalledWith(7)
  })
})
