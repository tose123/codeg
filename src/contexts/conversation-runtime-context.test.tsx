/**
 * Regression coverage for the per-conversation fetch-generation guard
 * that protects `FETCH_DETAIL_SUCCESS` / `FETCH_DETAIL_ERROR` from
 * out-of-order resolution and from resurrecting a removed session.
 *
 * The bug fixed by the generation counter:
 *
 *   1. Open dialog for child 99 → `refetchDetail(99)` issues fetch A.
 *   2. User closes the dialog → `removeConversation(99)` deletes state.
 *   3. Fetch A resolves AFTER the unmount → `FETCH_DETAIL_SUCCESS`
 *      reducer recreates the session with stale detail.
 *   4. User reopens → `useConversationDetail`'s active-data guard
 *      skips the auto-fetch because `session.detail` is set.
 *   5. The user is shown a stale pre-completion transcript.
 *
 * The counter also prevents a stale-response-wins race:
 *
 *   1. Open A → fetch A (slow).
 *   2. Close A.
 *   3. Open B → fetch B (faster).
 *   4. Fetch B resolves first — fresh detail in state.
 *   5. Fetch A resolves second — would overwrite B's fresh detail
 *      with stale, but the generation guard ignores it.
 */

import { act, render, screen } from "@testing-library/react"
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest"
import { useEffect, type ReactNode } from "react"

import {
  ConversationRuntimeProvider,
  useConversationRuntime,
} from "@/contexts/conversation-runtime-context"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  getFolderConversation: vi.fn(),
}))

const { getFolderConversation } = await import("@/lib/api")
const mockGetFolderConversation = vi.mocked(getFolderConversation)

function detailWithTitle(title: string): DbConversationDetail {
  return {
    summary: {
      id: 99,
      folder_id: 1,
      agent_type: "codex",
      title,
      status: "in_progress",
      model: null,
      git_branch: null,
      external_id: "ext-1",
      message_count: 0,
      created_at: "2026-05-28T00:00:00.000Z",
      updated_at: "2026-05-28T00:00:00.000Z",
    },
    turns: [],
    session_stats: null,
  }
}

let preserveLiveFlag = false

const LIVE_MSG: LiveMessage = {
  id: "lm-1",
  role: "assistant",
  content: [],
  startedAt: 0,
}

/** Probe component that exposes runtime actions to the test and lets it
 *  read back the session state via DOM attributes. */
function Probe() {
  const {
    refetchDetail,
    removeConversation,
    setLiveMessage,
    setLiveOwnsActiveTurn,
    getSession,
  } = useConversationRuntime()
  const session = getSession(99)
  return (
    <div>
      <button
        data-testid="refetch"
        type="button"
        onClick={() => refetchDetail(99)}
      >
        refetch
      </button>
      <button
        data-testid="refetch-preserve"
        type="button"
        onClick={() => refetchDetail(99, { preserveLive: preserveLiveFlag })}
      >
        refetch-preserve
      </button>
      <button
        data-testid="set-live"
        type="button"
        onClick={() => setLiveMessage(99, LIVE_MSG, true)}
      >
        set-live
      </button>
      <button
        data-testid="set-live-owns"
        type="button"
        onClick={() => setLiveOwnsActiveTurn(99, true)}
      >
        set-live-owns
      </button>
      <button
        data-testid="remove"
        type="button"
        onClick={() => removeConversation(99)}
      >
        remove
      </button>
      <div data-testid="title">
        {session?.detail?.summary.title ?? "no-detail"}
      </div>
      <div data-testid="has-session">{session ? "yes" : "no"}</div>
      <div data-testid="loading">{session?.detailLoading ? "yes" : "no"}</div>
      <div data-testid="has-live">{session?.liveMessage ? "yes" : "no"}</div>
      <div data-testid="live-owns">
        {session?.liveOwnsActiveTurn ? "yes" : "no"}
      </div>
    </div>
  )
}

function renderProvider(children: ReactNode = <Probe />) {
  return render(
    <ConversationRuntimeProvider>{children}</ConversationRuntimeProvider>
  )
}

describe("ConversationRuntimeProvider fetch-generation guard", () => {
  let originalConsoleError: typeof console.error
  let consoleErrorSpy: MockInstance

  beforeEach(() => {
    mockGetFolderConversation.mockReset()
    preserveLiveFlag = false
    originalConsoleError = console.error
    // Filter React's act() warnings produced when promise resolutions
    // commit asynchronously; the tests use act() correctly but the
    // microtask boundary is finer-grained than RTL's wrapper.
    consoleErrorSpy = vi.spyOn(console, "error").mockImplementation(() => {})
  })

  afterEach(() => {
    console.error = originalConsoleError
    consoleErrorSpy.mockRestore()
  })

  it("ignores a fetch response that resolves after removeConversation — no zombie session is created", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    mockGetFolderConversation.mockImplementationOnce(
      () =>
        new Promise<DbConversationDetail>((resolve) => {
          resolveA = resolve
        })
    )

    renderProvider()
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    expect(screen.getByTestId("loading").textContent).toBe("yes")

    // Tear down the session BEFORE fetch A resolves — simulates the user
    // closing the dialog while the detail is still loading.
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    expect(screen.getByTestId("has-session").textContent).toBe("no")

    // Fetch A resolves with stale detail AFTER removal. The
    // generation-counter guard must drop this resolution silently — no
    // FETCH_DETAIL_SUCCESS dispatched, so the session stays gone.
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("has-session").textContent).toBe("no")
    expect(screen.getByTestId("title").textContent).toBe("no-detail")
  })

  it("refetchDetail preserves a bridged live message when preserveLive:true, and wipes it on a plain load", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    let resolveB!: (detail: DbConversationDetail) => void
    mockGetFolderConversation
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveA = resolve
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveB = resolve
          })
      )

    renderProvider()

    // Bridge a live reply (isLive bypasses the SET_LIVE_MESSAGE guard).
    await act(async () => {
      screen.getByTestId("set-live").click()
    })
    expect(screen.getByTestId("has-live").textContent).toBe("yes")

    // preserveLive=true (child still streaming) → the load folds in the
    // persisted detail but keeps the bridged live reply.
    preserveLiveFlag = true
    await act(async () => {
      screen.getByTestId("refetch-preserve").click()
    })
    await act(async () => {
      resolveA(detailWithTitle("with-live"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("with-live")
    expect(screen.getByTestId("has-live").textContent).toBe("yes")

    // preserveLive=false (settled) → the next load is authoritative and wipes
    // the (now-promoted) live reply, matching the default FETCH_DETAIL_SUCCESS
    // behavior.
    preserveLiveFlag = false
    await act(async () => {
      screen.getByTestId("refetch-preserve").click()
    })
    await act(async () => {
      resolveB(detailWithTitle("no-live"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("no-live")
    expect(screen.getByTestId("has-live").textContent).toBe("no")
  })

  it("setLiveOwnsActiveTurn marks the session so getTimelineTurns strips persisted assistant turns while liveMessage is present", () => {
    renderProvider()
    // Initially no session.
    expect(screen.getByTestId("live-owns").textContent).toBe("no")
    // After marking, the session is created and the flag is set.
    act(() => {
      screen.getByTestId("set-live-owns").click()
    })
    expect(screen.getByTestId("live-owns").textContent).toBe("yes")
  })

  it("drops a stale fetch resolution that arrives after a fresh refetchDetail (fresh-wins regardless of order)", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    let resolveB!: (detail: DbConversationDetail) => void
    mockGetFolderConversation
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveA = resolve
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveB = resolve
          })
      )

    renderProvider()
    // First open — fetch A in flight.
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    // Close, then second open — fetch B in flight. Each refetchDetail
    // bumps the generation counter, so A's eventual resolution should
    // be ignored.
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    await act(async () => {
      screen.getByTestId("refetch").click()
    })

    // Resolve B FIRST — fresh detail lands.
    await act(async () => {
      resolveB(detailWithTitle("fresh-B"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")

    // Then resolve A — stale. Without the generation guard this would
    // overwrite fresh-B; with it, fresh-B stays put.
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")
  })

  it("a fresh fetch resolution after a stale one still wins (forward direction unchanged)", async () => {
    let resolveA!: (detail: DbConversationDetail) => void
    let resolveB!: (detail: DbConversationDetail) => void
    mockGetFolderConversation
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveA = resolve
          })
      )
      .mockImplementationOnce(
        () =>
          new Promise<DbConversationDetail>((resolve) => {
            resolveB = resolve
          })
      )

    renderProvider()
    await act(async () => {
      screen.getByTestId("refetch").click()
    })
    await act(async () => {
      screen.getByTestId("remove").click()
    })
    await act(async () => {
      screen.getByTestId("refetch").click()
    })

    // Resolve A first (stale, already invalidated by remove + new refetch).
    await act(async () => {
      resolveA(detailWithTitle("stale-A"))
      await Promise.resolve()
    })
    // A's resolution was ignored — title stays empty until B lands.
    expect(screen.getByTestId("title").textContent).toBe("no-detail")

    // Resolve B — fresh detail wins as the latest generation.
    await act(async () => {
      resolveB(detailWithTitle("fresh-B"))
      await Promise.resolve()
    })
    expect(screen.getByTestId("title").textContent).toBe("fresh-B")
  })
})

/**
 * `getTimelineTurns` memoizes per conversation by session reference, so a
 * dispatch that updates conversation A leaves conversation B's timeline array
 * referentially identical. This is what lets MessageListView's `threadItems`
 * useMemo short-circuit for every tab except the one whose session actually
 * changed — neutralizing the cross-tab broadcast fan-out without unmounting
 * any session (tile mode keeps every active conversation mounted).
 */
describe("ConversationRuntimeProvider getTimelineTurns memoization", () => {
  const runtimeHolder: {
    current: ReturnType<typeof useConversationRuntime> | undefined
  } = { current: undefined }

  function RuntimeCapture() {
    const runtime = useConversationRuntime()
    useEffect(() => {
      runtimeHolder.current = runtime
    })
    return null
  }

  function userTurn(id: string): MessageTurn {
    return {
      id,
      role: "user",
      blocks: [{ type: "text", text: id }],
      timestamp: "2026-05-28T00:00:00.000Z",
    }
  }

  beforeEach(() => {
    runtimeHolder.current = undefined
  })

  it("returns a stable reference for a conversation untouched by an unrelated update, and a fresh reference for the one that changed", () => {
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Seed two independent conversations.
    act(() => {
      api().appendOptimisticTurn(1, userTurn("a1"), "a1")
    })
    act(() => {
      api().appendOptimisticTurn(2, userTurn("b1"), "b1")
    })

    // Prime the cache for both.
    const timeline1Before = api().getTimelineTurns(1)
    const timeline2Before = api().getTimelineTurns(2)
    expect(timeline1Before).toHaveLength(1)
    expect(timeline2Before).toHaveLength(1)

    // Update only conversation 1.
    act(() => {
      api().appendOptimisticTurn(1, userTurn("a2"), "a2")
    })

    const timeline1After = api().getTimelineTurns(1)
    const timeline2After = api().getTimelineTurns(2)

    // Conversation 2 was untouched → identical array reference (cache hit).
    expect(timeline2After).toBe(timeline2Before)
    // Conversation 1 changed → new reference and new content.
    expect(timeline1After).not.toBe(timeline1Before)
    expect(timeline1After).toHaveLength(2)
  })

  it("returns a stable empty-array reference for an unknown conversation", () => {
    renderProvider(<RuntimeCapture />)
    const first = runtimeHolder.current!.getTimelineTurns(12345)
    const second = runtimeHolder.current!.getTimelineTurns(67890)
    expect(first).toHaveLength(0)
    expect(second).toBe(first)
  })
})

/**
 * Delegation-child viewer projection in `getTimelineTurns`. When the sub-agent
 * dialog marks a session `liveOwnsActiveTurn` and supplies the kickoff task:
 *   - the persisted copy of the reply is stripped while a live/local reply
 *     owns the turn (no partial-plus-stream duplicate), and
 *   - the kickoff USER turn is synthesized from the known task text while the
 *     async JSONL transcript still lags — then automatically replaced by the
 *     real persisted user turn once it lands (no duplicate, no cleanup).
 */
describe("ConversationRuntimeProvider delegation kickoff projection", () => {
  const runtimeHolder: {
    current: ReturnType<typeof useConversationRuntime> | undefined
  } = { current: undefined }

  function RuntimeCapture() {
    const runtime = useConversationRuntime()
    useEffect(() => {
      runtimeHolder.current = runtime
    })
    return null
  }

  function assistantTurn(id: string): MessageTurn {
    return {
      id,
      role: "assistant",
      blocks: [{ type: "text", text: id }],
      timestamp: "2026-05-28T00:00:00.000Z",
    }
  }

  function userTurn(id: string): MessageTurn {
    return {
      id,
      role: "user",
      blocks: [{ type: "text", text: id }],
      timestamp: "2026-05-28T00:00:00.000Z",
    }
  }

  function detailWithTurns(turns: MessageTurn[]): DbConversationDetail {
    return {
      summary: {
        id: 99,
        folder_id: 1,
        agent_type: "codex",
        title: "child",
        status: "in_progress",
        model: null,
        git_branch: null,
        external_id: "ext-1",
        message_count: turns.length,
        created_at: "2026-05-28T00:00:00.000Z",
        updated_at: "2026-05-28T00:00:00.000Z",
      },
      turns,
      session_stats: null,
    }
  }

  beforeEach(() => {
    runtimeHolder.current = undefined
    mockGetFolderConversation.mockReset()
  })

  it("synthesizes the kickoff user turn (and strips the persisted reply) while the transcript has no user turn yet", async () => {
    // DB lags: only a partial assistant turn is persisted, no user turn.
    mockGetFolderConversation.mockResolvedValueOnce(
      detailWithTurns([assistantTurn("a1")])
    )
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    act(() => {
      api().setLiveOwnsActiveTurn(99, true, "do the thing")
    })
    act(() => {
      api().setLiveMessage(99, LIVE_MSG, true)
    })
    await act(async () => {
      api().refetchDetail(99, { preserveLive: true })
      await Promise.resolve()
    })

    const timeline = api().getTimelineTurns(99)
    // First item is the synthesized kickoff user turn from the known task.
    expect(timeline[0].key).toBe("kickoff-99")
    expect(timeline[0].turn.role).toBe("user")
    expect(timeline[0].turn.blocks[0]).toMatchObject({
      type: "text",
      text: "do the thing",
    })
    // The persisted partial assistant turn is stripped (live owns the reply).
    expect(
      timeline.some(
        (t) => t.phase === "persisted" && t.turn.role === "assistant"
      )
    ).toBe(false)
  })

  it("uses the real persisted user turn instead of synthesizing once it has landed", async () => {
    mockGetFolderConversation.mockResolvedValueOnce(
      detailWithTurns([userTurn("u1"), assistantTurn("a1")])
    )
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    act(() => {
      api().setLiveOwnsActiveTurn(99, true, "do the thing")
    })
    act(() => {
      api().setLiveMessage(99, LIVE_MSG, true)
    })
    await act(async () => {
      api().refetchDetail(99, { preserveLive: true })
      await Promise.resolve()
    })

    const timeline = api().getTimelineTurns(99)
    // Exactly one user turn, and it's the authentic persisted one — no synthetic.
    const users = timeline.filter((t) => t.turn.role === "user")
    expect(users).toHaveLength(1)
    expect(users[0].turn.id).toBe("u1")
    expect(timeline.some((t) => t.key === "kickoff-99")).toBe(false)
  })

  it("keeps the adopted local reply and dedupes the persisted copy once [user, assistant] lands (reopen-after-completion)", async () => {
    // The persisted transcript catches up only after the adoption already ran.
    mockGetFolderConversation.mockResolvedValueOnce(
      detailWithTurns([userTurn("u1"), assistantTurn("a1")])
    )
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Simulate the adopt-settled-reply path the dialog runs on reopen: mark the
    // viewer, bridge the retained reply as live, promote it to a completed
    // local turn.
    const liveReply: LiveMessage = {
      id: "lr-1",
      role: "assistant",
      content: [{ type: "text", text: "final reply" }],
      startedAt: 0,
    }
    act(() => {
      api().setLiveOwnsActiveTurn(99, true, "do the thing")
    })
    act(() => {
      api().setLiveMessage(99, liveReply, true)
    })
    act(() => {
      api().completeTurn(99, liveReply)
    })
    await act(async () => {
      api().refetchDetail(99, { preserveLive: true })
      await Promise.resolve()
    })

    const timeline = api().getTimelineTurns(99)
    const users = timeline.filter((t) => t.turn.role === "user")
    const assistants = timeline.filter((t) => t.turn.role === "assistant")
    // Exactly one user (the real persisted one) and one assistant (the adopted
    // local reply; the persisted copy is stripped) — no duplication, no blank.
    expect(users).toHaveLength(1)
    expect(users[0].turn.id).toBe("u1")
    expect(assistants).toHaveLength(1)
    expect(timeline.some((t) => t.key === "kickoff-99")).toBe(false)
  })

  it("does not synthesize a kickoff for a normal (non-live-owned) session", async () => {
    mockGetFolderConversation.mockResolvedValueOnce(
      detailWithTurns([assistantTurn("a1")])
    )
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // No setLiveOwnsActiveTurn → ordinary panel. Even with a kickoff-less
    // assistant-only transcript, nothing is synthesized or stripped.
    await act(async () => {
      api().refetchDetail(99, { preserveLive: true })
      await Promise.resolve()
    })

    const timeline = api().getTimelineTurns(99)
    expect(timeline.some((t) => t.key === "kickoff-99")).toBe(false)
    expect(timeline.some((t) => t.turn.role === "assistant")).toBe(true)
  })
})

/**
 * Streaming/local turn dedup in `getTimelineTurns`. A premature or duplicate
 * COMPLETE_TURN (e.g. the background `turn_complete` listener in
 * ConversationDetailPanel racing the panel's own promotion) promotes a snapshot
 * of the in-flight turn into `localTurns` while the SAME liveMessage keeps
 * streaming and is re-bridged. Both are built from that one liveMessage, so
 * they share `live-<cid>-<liveMessageId>` turn ids. The timeline must surface
 * the turn exactly once (the live copy wins), never duplicated — otherwise
 * `mergeConsecutiveAssistantTurns` flat-maps the same parts twice and React
 * throws `Encountered two children with the same key, tc-<toolCallId>`.
 */
describe("ConversationRuntimeProvider streaming/local turn dedup", () => {
  const runtimeHolder: {
    current: ReturnType<typeof useConversationRuntime> | undefined
  } = { current: undefined }

  function RuntimeCapture() {
    const runtime = useConversationRuntime()
    useEffect(() => {
      runtimeHolder.current = runtime
    })
    return null
  }

  beforeEach(() => {
    runtimeHolder.current = undefined
  })

  it("drops the promoted snapshot when the same liveMessage is still streaming (no duplicate turn id)", () => {
    const liveMsg: LiveMessage = {
      id: "lm-dup",
      role: "assistant",
      content: [{ type: "text", text: "streaming reply" }],
      startedAt: 0,
    }
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Bridge the live turn, promote it (the premature COMPLETE_TURN), then the
    // mirror effect re-bridges the SAME liveMessage while still "streaming".
    act(() => {
      api().setLiveMessage(99, liveMsg, true)
    })
    act(() => {
      api().completeTurn(99, liveMsg)
    })
    act(() => {
      api().setLiveMessage(99, liveMsg, true)
    })

    const timeline = api().getTimelineTurns(99)
    const ids = timeline.map((t) => t.turn.id)
    // The turn id appears exactly once; the duplicate localTurns snapshot is
    // filtered out and the streaming copy survives.
    expect(ids.filter((id) => id === "live-99-lm-dup")).toHaveLength(1)
    expect(new Set(ids).size).toBe(ids.length)
    expect(timeline.find((t) => t.turn.id === "live-99-lm-dup")?.phase).toBe(
      "streaming"
    )
  })

  it("keeps both turns when a completed turn and a different streaming turn coexist (distinct ids, no false dedup)", () => {
    const turnA: LiveMessage = {
      id: "lm-a",
      role: "assistant",
      content: [{ type: "text", text: "turn A" }],
      startedAt: 0,
    }
    const turnB: LiveMessage = {
      id: "lm-b",
      role: "assistant",
      content: [{ type: "text", text: "turn B" }],
      startedAt: 0,
    }
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Turn A streams then completes (promoted to localTurns, liveMessage cleared
    // by COMPLETE_TURN); turn B then starts streaming with a fresh liveMessage.
    act(() => {
      api().setLiveMessage(99, turnA, true)
    })
    act(() => {
      api().completeTurn(99, turnA)
    })
    act(() => {
      api().setLiveMessage(99, turnB, true)
    })

    const timeline = api().getTimelineTurns(99)
    const assistantIds = timeline
      .filter((t) => t.turn.role === "assistant")
      .map((t) => t.turn.id)
    // Both turns survive — distinct liveMessage ids never collide.
    expect(assistantIds).toContain("live-99-lm-a")
    expect(assistantIds).toContain("live-99-lm-b")
    expect(new Set(assistantIds).size).toBe(assistantIds.length)
  })

  it("does not accumulate duplicate localTurns when the same live turn is re-promoted after a re-bridge (final completion, liveMessage cleared)", () => {
    const liveMsg: LiveMessage = {
      id: "lm-dup2",
      role: "assistant",
      content: [{ type: "text", text: "streaming reply" }],
      startedAt: 0,
    }
    renderProvider(<RuntimeCapture />)
    const api = () => runtimeHolder.current!

    // Premature promote, re-bridge of the SAME liveMessage, then a final
    // promote. COMPLETE_TURN must not append a second copy of the turn, and the
    // final promote clears liveMessage so there is no streaming turn left to
    // filter against — the dedup has to already hold in localTurns.
    act(() => {
      api().setLiveMessage(99, liveMsg, true)
    })
    act(() => {
      api().completeTurn(99, liveMsg)
    })
    act(() => {
      api().setLiveMessage(99, liveMsg, true)
    })
    act(() => {
      api().completeTurn(99, liveMsg)
    })

    const session = api().getSession(99)
    // liveMessage is cleared by the final COMPLETE_TURN…
    expect(session?.liveMessage).toBeNull()
    // …and localTurns holds the turn exactly once (no re-promotion duplicate).
    expect(
      session?.localTurns.filter((t) => t.id === "live-99-lm-dup2")
    ).toHaveLength(1)

    const ids = api()
      .getTimelineTurns(99)
      .map((t) => t.turn.id)
    expect(ids.filter((id) => id === "live-99-lm-dup2")).toHaveLength(1)
    expect(new Set(ids).size).toBe(ids.length)
  })
})
