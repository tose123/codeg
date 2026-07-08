/**
 * Background-task lifecycle for the Agent tool card.
 *
 * The Claude parser rewrites an async sub-agent launch ack's output — internal
 * metadata text never meant for users — into a structured marker payload
 * (`BACKGROUND_TASK_MARKER` + one-line JSON), joined with the latest matching
 * `<task-notification>` from the same transcript (see
 * `ClaudeRecordAccumulator::finalize_background_lifecycle` in
 * `src-tauri/src/parsers/claude.rs`). This module is the frontend side of that
 * contract.
 *
 * A `null` status means no notification has been observed in the transcript —
 * deliberately rendered as "launched · result pending", NEVER as "running":
 * the transcript alone cannot distinguish a still-running task from one whose
 * CLI died (the zombie-"running" trap). Live "running" presentation is only
 * derived from the in-flight wire ack text (`isAsyncLaunchAckText`), which by
 * construction only exists inside a live session.
 */

export const BACKGROUND_TASK_MARKER = "[[codeg-background-task]]"

export interface BackgroundTaskLifecycle {
  taskId: string
  /** `<status>` of the latest task-notification ("completed" on success);
   *  `null` while no notification has been observed. */
  status: string | null
  summary: string | null
  /** The notification's `<result>` markdown (parser-capped). */
  result: string | null
}

/** Parse a parser-rewritten lifecycle marker out of a tool output preview.
 *  Returns `null` for anything that isn't a well-formed marker. */
export function parseBackgroundTaskMarker(
  output: string | null | undefined
): BackgroundTaskLifecycle | null {
  if (!output) return null
  const trimmed = output.trimStart()
  if (!trimmed.startsWith(BACKGROUND_TASK_MARKER)) return null
  try {
    const payload = JSON.parse(
      trimmed.slice(BACKGROUND_TASK_MARKER.length)
    ) as Record<string, unknown>
    const taskId = typeof payload.task_id === "string" ? payload.task_id : null
    if (!taskId) return null
    return {
      taskId,
      status: typeof payload.status === "string" ? payload.status : null,
      summary: typeof payload.summary === "string" ? payload.summary : null,
      result: typeof payload.result === "string" ? payload.result : null,
    }
  } catch {
    return null
  }
}

/**
 * Whether a LIVE wire tool output is the async sub-agent launch ack
 * ("Async agent launched successfully. … You will be notified…").
 * Presentation-only: used to show a "running in background" state instead of
 * dumping the internal ack text while the turn's wire data is still what the
 * card renders (the parser marker replaces it on the next transcript parse).
 */
export function isAsyncLaunchAckText(
  output: string | null | undefined
): boolean {
  if (!output) return false
  return output.includes("Async agent launched successfully")
}
