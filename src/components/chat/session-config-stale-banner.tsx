"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { AlertTriangle, RefreshCw, X } from "lucide-react"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { useConnection } from "@/hooks/use-connection"
import { cn } from "@/lib/utils"

/**
 * Per-conversation banner shown at the top of a session panel when the agent's
 * effective settings changed AFTER the session spawned, so the running process
 * is still on its launch-time config. In the tiled layout each stale session
 * renders its own banner, so the user can spot and resolve them one by one.
 *
 * Behaviour:
 * - Owners only — viewers don't own the backend process and can't restart it.
 * - "Restart to apply" disconnects + resumes the same session (history kept),
 *   so the new process reads current config and the banner clears.
 * - Disabled while a turn is in flight (`prompting`) — restarting would
 *   interrupt it — with a tooltip explaining why.
 * - The X dismisses the banner for the CURRENT drift only; a later settings
 *   change re-shows it.
 *
 * Returns null (no layout impact) when there's nothing to show.
 */
export function SessionConfigStaleBanner({
  contextKey,
}: {
  contextKey: string
}) {
  const t = useTranslations("Folder.chat.configStale")
  const {
    configStale,
    configStaleKind,
    configStaleDismissed,
    isViewer,
    isDelegationChild,
    status,
    reapplyConfig,
    dismissConfigStale,
  } = useConnection(contextKey)
  const [restarting, setRestarting] = useState(false)

  // Owners only: viewers and delegation children don't own the backend process,
  // so "restart to apply" isn't theirs to do.
  if (!configStale || configStaleDismissed || isViewer || isDelegationChild)
    return null

  const turnInFlight = status === "prompting"
  // `connecting` covers the reconnect that `reapplyConfig` itself triggers.
  const reconnecting = restarting || status === "connecting"
  const restartDisabled = turnInFlight || reconnecting

  const title =
    configStaleKind === "model_provider"
      ? t("modelProviderTitle")
      : t("agentConfigTitle")

  const handleRestart = async () => {
    if (restartDisabled) return
    setRestarting(true)
    try {
      const restarted = await reapplyConfig()
      if (restarted) {
        toast.success(t("applied"))
        // On success the session restarts and `configStale` clears, which
        // unmounts this banner — no need to reset `restarting`.
      } else {
        // No-op (connection vanished mid-click, etc.) — don't claim "applied".
        setRestarting(false)
      }
    } catch (error) {
      toast.error(t("restartFailed"), {
        description: error instanceof Error ? error.message : String(error),
      })
      setRestarting(false)
    }
  }

  return (
    <div className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-xs text-amber-700 dark:text-amber-300">
      <div className="mx-auto flex w-full max-w-3xl items-center gap-2">
        <AlertTriangle className="h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
        <div className="min-w-0 flex-1 leading-snug">
          <span className="font-medium">{title}</span>
          <span className="ml-1 text-amber-700/80 dark:text-amber-300/80">
            {t("description")}
          </span>
        </div>
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              {/* Wrapper span so the tooltip still fires while the button is
                  disabled (disabled elements don't emit pointer events). */}
              <span className="shrink-0">
                <Button
                  size="sm"
                  variant="outline"
                  className="h-7 gap-1.5 border-amber-500/40 bg-transparent text-amber-700 hover:bg-amber-500/20 hover:text-amber-800 dark:text-amber-300 dark:hover:text-amber-200"
                  disabled={restartDisabled}
                  onClick={handleRestart}
                >
                  <RefreshCw
                    className={cn(
                      "h-3.5 w-3.5",
                      reconnecting && "animate-spin"
                    )}
                  />
                  {reconnecting ? t("restarting") : t("restart")}
                </Button>
              </span>
            </TooltipTrigger>
            {turnInFlight && (
              <TooltipContent>{t("restartDisabledDuringTurn")}</TooltipContent>
            )}
          </Tooltip>
        </TooltipProvider>
        <Button
          size="icon"
          variant="ghost"
          className="h-6 w-6 shrink-0 text-amber-700/70 hover:bg-amber-500/20 hover:text-amber-800 dark:text-amber-300/70 dark:hover:text-amber-200"
          onClick={dismissConfigStale}
          aria-label={t("dismiss")}
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  )
}
