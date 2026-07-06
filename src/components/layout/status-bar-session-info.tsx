"use client"

import { useMemo } from "react"
import { GitBranch } from "lucide-react"
import { useTabStore } from "@/contexts/tab-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"

export function StatusBarSessionInfo() {
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)

  const activeTab = useMemo(
    () => tabs.find((t) => t.id === activeTabId) ?? null,
    [tabs, activeTabId]
  )

  // Selecting the matching summary (not the whole list) keeps this component
  // inert to unrelated conversation updates: `find` returns the same object
  // reference until this conversation itself changes.
  const summary = useAppWorkspaceStore((s) => {
    if (!activeTab || activeTab.kind !== "conversation") return null
    return (
      s.conversations.find(
        (c) =>
          c.id === activeTab.conversationId &&
          c.agent_type === activeTab.agentType
      ) ?? null
    )
  })

  if (!summary) return null

  return (
    <div className="flex items-center gap-4">
      {summary.git_branch && (
        <span className="flex items-center gap-1">
          <GitBranch className="h-3 w-3" />
          {summary.git_branch}
        </span>
      )}
    </div>
  )
}
