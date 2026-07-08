import { describe, expect, it } from "vitest"
import {
  BACKGROUND_TASK_MARKER,
  isAsyncLaunchAckText,
  parseBackgroundTaskMarker,
} from "@/lib/background-agent"

describe("parseBackgroundTaskMarker", () => {
  it("parses a settled marker", () => {
    const output = `${BACKGROUND_TASK_MARKER}{"task_id":"abc123","status":"completed","summary":"Agent \\"Run pnpm build\\" finished","result":"Build OK"}`
    expect(parseBackgroundTaskMarker(output)).toEqual({
      taskId: "abc123",
      status: "completed",
      summary: 'Agent "Run pnpm build" finished',
      result: "Build OK",
    })
  })

  it("parses an unsettled marker (null status — never rendered as running)", () => {
    const output = `${BACKGROUND_TASK_MARKER}{"task_id":"nores99","status":null,"summary":null,"result":null}`
    expect(parseBackgroundTaskMarker(output)).toEqual({
      taskId: "nores99",
      status: null,
      summary: null,
      result: null,
    })
  })

  it("rejects non-marker output, malformed JSON, and missing task_id", () => {
    expect(parseBackgroundTaskMarker(null)).toBeNull()
    expect(parseBackgroundTaskMarker("plain tool output")).toBeNull()
    expect(
      parseBackgroundTaskMarker(`${BACKGROUND_TASK_MARKER}{not json`)
    ).toBeNull()
    expect(
      parseBackgroundTaskMarker(
        `${BACKGROUND_TASK_MARKER}{"status":"completed"}`
      )
    ).toBeNull()
  })
})

describe("isAsyncLaunchAckText", () => {
  it("matches the live wire ack and nothing else", () => {
    expect(
      isAsyncLaunchAckText(
        "Async agent launched successfully. (This tool result is internal metadata…)\nagentId: a793c…"
      )
    ).toBe(true)
    expect(isAsyncLaunchAckText("Sub-agent finished: all tests pass")).toBe(
      false
    )
    expect(isAsyncLaunchAckText(null)).toBe(false)
  })
})
