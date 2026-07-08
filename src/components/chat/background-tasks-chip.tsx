"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"
import { useConnection } from "@/hooks/use-connection"

/**
 * How long the "syncing results" state stays visible after a settlement with
 * no follow-up turns observed. The agent's reaction normally lands within
 * 3–15s (model time-to-first-block); past this window the CLI was likely
 * killed and the indicator must not strand.
 */
const SETTLE_SYNC_DISPLAY_MS = 30_000

/**
 * Slim per-conversation strip shown while this connection has
 * launched-but-unresolved background work (async sub-agents / background
 * shell tasks, accounted from the agent's own transcript by the backend
 * watcher). Makes the otherwise-silent gap perceivable: the turn already
 * ended, but results will still stream in as overlay turns and the
 * connection is being kept alive for them.
 *
 * After the last task settles the strip doesn't vanish into a void: it
 * transitions to a "syncing results" state until the agent's reaction turn
 * starts surfacing (or the display window above expires).
 *
 * Returns null (no layout impact) when nothing is pending.
 */
export function BackgroundTasksChip({ contextKey }: { contextKey: string }) {
  const t = useTranslations("Folder.chat.backgroundTasks")
  const { backgroundOutstanding, backgroundSettleSyncingSince } =
    useConnection(contextKey)

  // Which arm timestamp has display-expired. Tied to the specific value so a
  // re-arm (another settlement → fresh timestamp) un-expires automatically,
  // and render stays pure (Date.now() only runs inside the effect).
  const [expiredFor, setExpiredFor] = useState<number | null>(null)
  useEffect(() => {
    if (backgroundSettleSyncingSince == null) return
    const remaining =
      SETTLE_SYNC_DISPLAY_MS - (Date.now() - backgroundSettleSyncingSince)
    // An already-expired arm (e.g. hydrated stale) fires on the next tick —
    // never synchronously in the effect body.
    const timer = setTimeout(
      () => setExpiredFor(backgroundSettleSyncingSince),
      Math.max(0, remaining) + 50
    )
    return () => clearTimeout(timer)
  }, [backgroundSettleSyncingSince])

  const showSyncing =
    backgroundOutstanding <= 0 &&
    backgroundSettleSyncingSince != null &&
    expiredFor !== backgroundSettleSyncingSince

  if (backgroundOutstanding <= 0 && !showSyncing) return null

  return (
    <div className="border-b border-sky-500/20 bg-sky-500/10 px-3 py-1.5 text-xs text-sky-700 dark:text-sky-300">
      <div className="mx-auto flex w-full max-w-3xl items-center justify-center gap-2">
        {/* Plain animate-spin on purpose: a motion-safe gate would freeze the
            only "still working" signal for Reduce Motion users. */}
        <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" />
        <span className="min-w-0 truncate">
          {backgroundOutstanding > 0
            ? t("running", { count: backgroundOutstanding })
            : t("settling")}
        </span>
      </div>
    </div>
  )
}
