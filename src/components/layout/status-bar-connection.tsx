"use client"

import { useCallback, useSyncExternalStore } from "react"
import { Unplug } from "lucide-react"
import { useTranslations } from "next-intl"
import { useConnectionStore } from "@/contexts/acp-connections-context"
import { useTabStore } from "@/contexts/tab-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { AgentIcon } from "@/components/agent-icon"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { AGENT_LABELS } from "@/lib/types"
import { cn } from "@/lib/utils"

type ConnectionStatusLabelKey =
  | "connected"
  | "connecting"
  | "prompting"
  | "error"

const STATUS_STYLE: Record<
  string,
  { className: string; labelKey: ConnectionStatusLabelKey }
> = {
  connected: { className: "opacity-100", labelKey: "connected" },
  connecting: {
    className: "opacity-100 animate-pulse",
    labelKey: "connecting",
  },
  prompting: {
    className: "opacity-100 animate-pulse",
    labelKey: "prompting",
  },
  error: { className: "opacity-50", labelKey: "error" },
}

export function StatusBarConnection() {
  const t = useTranslations("Folder.statusBar.connection")
  const store = useConnectionStore()
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)

  // Subscribe to activeKey changes
  const subscribeActiveKey = useCallback(
    (cb: () => void) => store.subscribeActiveKey(cb),
    [store]
  )
  const getActiveKey = useCallback(() => store.getActiveKey(), [store])
  const activeKey = useSyncExternalStore(
    subscribeActiveKey,
    getActiveKey,
    getActiveKey
  )

  // Subscribe to the active connection's changes
  const subscribeConn = useCallback(
    (cb: () => void) => {
      if (!activeKey) return () => {}
      return store.subscribeKey(activeKey, cb)
    },
    [store, activeKey]
  )
  const getConnSnapshot = useCallback(
    () => (activeKey ? store.getConnection(activeKey) : undefined),
    [store, activeKey]
  )
  const activeConn = useSyncExternalStore(
    subscribeConn,
    getConnSnapshot,
    getConnSnapshot
  )

  const status = activeConn?.status ?? null
  const agentType = activeConn?.agentType ?? null

  // Selecting the primitive model string keeps this component inert to every
  // unrelated conversation update.
  const model = useAppWorkspaceStore((s) => {
    const tab = tabs.find((t) => t.id === activeTabId)
    if (!tab || tab.kind !== "conversation") return null
    const conv = s.conversations.find(
      (c) => c.id === tab.conversationId && c.agent_type === tab.agentType
    )
    return conv?.model ?? null
  })

  if (!agentType || !status || status === "disconnected") {
    return (
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <div className="flex items-center gap-1.5">
              <Unplug className="h-3.5 w-3.5 text-muted-foreground/50" />
              {model && <span>{model}</span>}
            </div>
          </TooltipTrigger>
          <TooltipContent side="top">{t("disconnected")}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
    )
  }

  const style = STATUS_STYLE[status]
  if (!style) return null

  const label = AGENT_LABELS[agentType]
  const statusLabel = t(style.labelKey)
  const tooltipText =
    status === "error" && activeConn?.error
      ? t("tooltipError", { agent: label, error: activeConn.error })
      : t("tooltip", { agent: label, status: statusLabel })

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <div className="flex items-center gap-1">
            <AgentIcon
              agentType={agentType}
              className={cn("size-3", style.className)}
            />
            {model && <span>{model}</span>}
          </div>
        </TooltipTrigger>
        <TooltipContent side="top">{tooltipText}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
