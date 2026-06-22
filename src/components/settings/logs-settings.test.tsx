import { type ReactNode, type Ref, useEffect, useImperativeHandle } from "react"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getLogSettings: vi.fn(),
  getRecentLogs: vi.fn(),
  listLogFiles: vi.fn(),
  openLogsDir: vi.fn(),
  readLogFile: vi.fn(),
  setLogSettings: vi.fn(),
  subscribeLogAppended: vi.fn(),
  subscribeLogSettingsChanged: vi.fn(),
}))

vi.mock("@/lib/platform", () => ({
  isDesktop: vi.fn(() => true),
  revealItemInDir: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() },
}))

vi.mock("@/lib/app-error", () => ({
  toErrorMessage: (e: unknown) => String(e),
}))

// virtua renders 0 rows in jsdom (no layout); render every child so findByText
// works, and expose a settable VirtualizerHandle. The viewer uses the
// array-children form, so `children` is already the LogRow elements.
const virtuaCtl = vi.hoisted(() => ({ scrollToIndex: vi.fn() }))
vi.mock("virtua", () => ({
  Virtualizer: ({
    children,
    ref,
  }: {
    children?: ReactNode
    ref?: Ref<unknown>
  }) => {
    useImperativeHandle(ref, () => ({
      scrollToIndex: virtuaCtl.scrollToIndex,
      scrollToOffset: () => {},
    }))
    return <>{children}</>
  },
}))

// The list mounts the Virtualizer only once OverlayScrollbars surfaces its
// viewport; the mock fires that bridge synchronously after mount.
vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({
    children,
    onViewportRef,
  }: {
    children?: ReactNode
    onViewportRef?: (el: HTMLElement | null) => void
  }) => {
    useEffect(() => {
      onViewportRef?.(document.createElement("div"))
    }, [onViewportRef])
    return <>{children}</>
  },
}))

import { LogsSettings } from "./logs-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  getLogSettings,
  getRecentLogs,
  setLogSettings,
  subscribeLogAppended,
  subscribeLogSettingsChanged,
} from "@/lib/api"
import type { LogRecord } from "@/lib/types"

const mockGetSettings = vi.mocked(getLogSettings)
const mockGetRecent = vi.mocked(getRecentLogs)
const mockSetSettings = vi.mocked(setLogSettings)
const mockSubAppended = vi.mocked(subscribeLogAppended)
const mockSubSettings = vi.mocked(subscribeLogSettingsChanged)

const M = enMessages.LogsSettings

function rec(
  seq: number,
  level: string,
  target: string,
  message: string,
  extra: Partial<LogRecord> = {}
): LogRecord {
  return {
    seq,
    timestamp_ms: 1_700_000_000_000 + seq,
    level,
    target,
    message,
    fields: {},
    spans: [],
    ...extra,
  }
}

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <LogsSettings />
    </NextIntlClientProvider>
  )
}

let appendedHandler: ((r: LogRecord) => void) | undefined

// Controllable requestAnimationFrame so we can assert the live-tail batching
// (many events → one flushed commit).
let rafCb: FrameRequestCallback | null = null
let rafScheduleCount = 0
async function flushRaf() {
  await act(async () => {
    const cb = rafCb
    rafCb = null
    cb?.(0)
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  appendedHandler = undefined
  rafCb = null
  rafScheduleCount = 0
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    rafCb = cb
    rafScheduleCount++
    return 1
  })
  vi.stubGlobal("cancelAnimationFrame", () => {
    rafCb = null
  })
  mockGetSettings.mockResolvedValue({
    level: "info",
    targets: [],
    env_locked: false,
  })
  mockGetRecent.mockResolvedValue([])
  mockSetSettings.mockResolvedValue({ level: "info", targets: [] })
  mockSubSettings.mockResolvedValue(() => {})
  mockSubAppended.mockImplementation(async (handler) => {
    appendedHandler = handler
    return () => {}
  })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("LogsSettings", () => {
  it("renders recent log records", async () => {
    mockGetRecent.mockResolvedValue([
      rec(1, "ERROR", "acp", "boom happened"),
      rec(2, "INFO", "web", "server started"),
    ])
    renderWithIntl()
    expect(await screen.findByText("boom happened")).toBeInTheDocument()
    expect(screen.getByText("server started")).toBeInTheDocument()
  })

  it("filters displayed records by search text", async () => {
    mockGetRecent.mockResolvedValue([
      rec(1, "ERROR", "acp", "boom happened"),
      rec(2, "INFO", "web", "server started"),
    ])
    renderWithIntl()
    await screen.findByText("boom happened")

    fireEvent.change(screen.getByPlaceholderText(M.searchPlaceholder), {
      target: { value: "boom" },
    })

    expect(screen.getByText("boom happened")).toBeInTheDocument()
    expect(screen.queryByText("server started")).not.toBeInTheDocument()
  })

  it("appends live-tailed records (coalesced via rAF)", async () => {
    mockGetRecent.mockResolvedValue([rec(1, "INFO", "web", "first record")])
    renderWithIntl()
    await screen.findByText("first record")
    await waitFor(() => expect(appendedHandler).toBeDefined())

    await act(async () => {
      appendedHandler?.(rec(2, "WARN", "acp", "live arrived"))
    })
    await flushRaf()

    expect(await screen.findByText("live arrived")).toBeInTheDocument()
  })

  it("coalesces a burst of appended records into a single flush", async () => {
    renderWithIntl()
    await waitFor(() => expect(appendedHandler).toBeDefined())

    await act(async () => {
      appendedHandler?.(rec(1, "INFO", "web", "alpha"))
      appendedHandler?.(rec(2, "INFO", "web", "bravo"))
      appendedHandler?.(rec(3, "INFO", "web", "charlie"))
    })
    // Three events, one scheduled frame.
    expect(rafScheduleCount).toBe(1)

    await flushRaf()
    expect(screen.getByText("alpha")).toBeInTheDocument()
    expect(screen.getByText("bravo")).toBeInTheDocument()
    expect(screen.getByText("charlie")).toBeInTheDocument()
  })

  it("re-schedules flushes after toggling live tail off and back on", async () => {
    renderWithIntl()
    await waitFor(() => expect(appendedHandler).toBeDefined())

    // Append (schedules a frame) then pause mid-pending: cleanup must cancel AND
    // reset the rAF id so a later resume can schedule again.
    await act(async () => {
      appendedHandler?.(rec(1, "INFO", "web", "before"))
    })
    expect(rafScheduleCount).toBe(1)

    fireEvent.click(screen.getByRole("button", { name: M.pause }))
    fireEvent.click(screen.getByRole("button", { name: M.resume }))
    await waitFor(() => expect(mockSubAppended).toHaveBeenCalledTimes(2))

    rafScheduleCount = 0
    await act(async () => {
      appendedHandler?.(rec(2, "INFO", "web", "after toggle"))
    })
    // A fresh frame is scheduled (would be 0 if rafRef held a stale id).
    expect(rafScheduleCount).toBe(1)
    await flushRaf()
    expect(screen.getByText("after toggle")).toBeInTheDocument()
  })

  it("expands a record to show its fields and span chain", async () => {
    mockGetRecent.mockResolvedValue([
      rec(1, "INFO", "web", "request done", {
        fields: { user_id: "7" },
        spans: [{ name: "http", fields: { path: "/x" } }],
      }),
    ])
    renderWithIntl()
    await screen.findByText("request done")

    fireEvent.click(screen.getByRole("button", { name: M.toggleDetails }))

    expect(screen.getByText("user_id")).toBeInTheDocument()
    expect(screen.getByText("7")).toBeInTheDocument()
    expect(screen.getByText(/http\{path=\/x\}/)).toBeInTheDocument()
  })

  it("clears the view", async () => {
    mockGetRecent.mockResolvedValue([rec(1, "INFO", "web", "to be cleared")])
    renderWithIntl()
    await screen.findByText("to be cleared")

    fireEvent.click(screen.getByRole("button", { name: M.clear }))

    expect(screen.queryByText("to be cleared")).not.toBeInTheDocument()
    expect(screen.getByText(M.empty)).toBeInTheDocument()
  })

  it("adds a per-module override and persists it", async () => {
    renderWithIntl()
    await screen.findByText(M.targetsTitle)

    fireEvent.click(screen.getByRole("button", { name: M.targetsAdd }))

    const input = screen.getByPlaceholderText("codeg_lib::acp")
    fireEvent.change(input, { target: { value: "codeg_lib::acp" } })
    fireEvent.blur(input)

    await waitFor(() =>
      expect(mockSetSettings).toHaveBeenCalledWith({
        level: "info",
        targets: [{ target: "codeg_lib::acp", level: "debug" }],
      })
    )
  })

  it("disables the override editor when env-locked", async () => {
    mockGetSettings.mockResolvedValue({
      level: "debug",
      targets: [],
      env_locked: true,
    })
    renderWithIntl()
    await screen.findByText(M.targetsTitle)

    expect(screen.getByRole("button", { name: M.targetsAdd })).toBeDisabled()
  })
})
