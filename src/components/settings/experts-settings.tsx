"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  Bot,
  Bug,
  CheckCheck,
  FileCode2,
  FlaskConical,
  FolderOpen,
  GitBranch,
  GitFork,
  GitMerge,
  Lightbulb,
  ListTodo,
  Loader2,
  MessageSquareQuote,
  MessageSquareReply,
  PlayCircle,
  RefreshCw,
  Sparkles,
  type LucideIcon,
} from "lucide-react"
import { useLocale, useTranslations } from "next-intl"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { Switch } from "@/components/ui/switch"
import { cn } from "@/lib/utils"
import {
  acpListAgents,
  expertsGetInstallStatus,
  expertsLinkToAgent,
  expertsList,
  expertsOpenCentralDir,
  expertsReadContent,
  expertsUnlinkFromAgent,
  openFolder,
} from "@/lib/api"
import { revealItemInDir } from "@/lib/platform"
import { getActiveRemoteConnectionId, isDesktop } from "@/lib/transport"
import { invalidateAgentExpertsCache } from "@/hooks/use-agent-experts"
import type {
  AcpAgentInfo,
  AgentType,
  ExpertInstallStatus,
  ExpertLinkState,
  ExpertListItem,
} from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"

const ICON_MAP: Record<string, LucideIcon> = {
  Lightbulb,
  ListTodo,
  PlayCircle,
  Bot,
  GitFork,
  GitBranch,
  FlaskConical,
  CheckCheck,
  Bug,
  MessageSquareQuote,
  MessageSquareReply,
  GitMerge,
  Sparkles,
  FileCode2,
}

const CATEGORY_SORT: Record<string, number> = {
  discovery: 1,
  planning: 2,
  execution: 3,
  quality: 4,
  debugging: 5,
  review: 6,
  meta: 7,
}

const LEFT_MIN_WIDTH = 320
const RIGHT_MIN_WIDTH = 440

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

function toPercent(pixels: number, totalPixels: number): number {
  if (totalPixels <= 0) return 0
  return (pixels / totalPixels) * 100
}

/**
 * next-intl locales are lower-case underscored like `zh_cn`. Our expert
 * metadata dictionary uses BCP47-ish keys like `zh-CN`. Normalize both
 * sides and fall back to `en`.
 */
function pickLocalized(
  dict: Record<string, string> | undefined,
  locale: string
): string {
  if (!dict) return ""
  if (dict[locale]) return dict[locale]
  const normalized = locale.replace("_", "-")
  if (dict[normalized]) return dict[normalized]
  const [lang] = normalized.split("-")
  const match = Object.keys(dict).find(
    (key) => key.toLowerCase().split("-")[0] === lang.toLowerCase()
  )
  if (match) return dict[match]
  return dict.en ?? Object.values(dict)[0] ?? ""
}

function stripFrontmatter(content: string): string {
  const match = content.match(/^---\s*\r?\n[\s\S]*?\r?\n---\s*(?:\r?\n)?/)
  if (!match) return content
  return content.slice(match[0].length)
}

function getIcon(name: string | null | undefined): LucideIcon {
  if (name && ICON_MAP[name]) return ICON_MAP[name]
  return Sparkles
}

export function ExpertsSettings() {
  const t = useTranslations("ExpertsSettings")
  const locale = useLocale()
  const panelContainerRef = useRef<HTMLDivElement | null>(null)
  const [panelContainerWidth, setPanelContainerWidth] = useState(0)

  const [experts, setExperts] = useState<ExpertListItem[]>([])
  const [agents, setAgents] = useState<AcpAgentInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [selectedExpertId, setSelectedExpertId] = useState<string | null>(null)
  const [searchQuery, setSearchQuery] = useState("")

  const [content, setContent] = useState<string>("")
  const [contentLoading, setContentLoading] = useState(false)

  const [statuses, setStatuses] = useState<Record<string, ExpertInstallStatus>>(
    {}
  )
  const [statusLoading, setStatusLoading] = useState(false)
  const [pendingMutation, setPendingMutation] = useState<string | null>(null)

  const translatedCategory = useCallback(
    (category: string): string => {
      switch (category) {
        case "discovery":
          return t("categories.discovery")
        case "planning":
          return t("categories.planning")
        case "execution":
          return t("categories.execution")
        case "quality":
          return t("categories.quality")
        case "debugging":
          return t("categories.debugging")
        case "review":
          return t("categories.review")
        case "meta":
          return t("categories.meta")
        default:
          return category
      }
    },
    [t]
  )

  const translatedState = useCallback(
    (state: ExpertLinkState): string => {
      switch (state) {
        case "not_linked":
          return t("states.not_linked")
        case "linked_to_codeg":
          return t("states.linked_to_codeg")
        case "linked_elsewhere":
          return t("states.linked_elsewhere")
        case "blocked_by_real_directory":
          return t("states.blocked_by_real_directory")
        case "broken":
          return t("states.broken")
        default:
          return state
      }
    },
    [t]
  )

  const refresh = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const [expertList, agentList] = await Promise.all([
        expertsList(),
        acpListAgents(),
      ])
      setExperts(expertList)
      setAgents(agentList)
    } catch (err) {
      const message = toErrorMessage(err)
      setLoadError(message)
      setExperts([])
      setAgents([])
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    refresh().catch((err) => {
      console.error("[ExpertsSettings] initial refresh failed:", err)
    })
  }, [refresh])

  useEffect(() => {
    const container = panelContainerRef.current
    if (!container) return
    const updateWidth = (next: number) => {
      setPanelContainerWidth((prev) =>
        Math.abs(prev - next) < 1 ? prev : next
      )
    }
    updateWidth(container.getBoundingClientRect().width)
    const observer = new ResizeObserver((entries) => {
      updateWidth(
        entries[0]?.contentRect.width ?? container.getBoundingClientRect().width
      )
    })
    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [])

  const sortedExperts = useMemo(() => {
    return [...experts].sort((a, b) => {
      const ca = CATEGORY_SORT[a.metadata.category] ?? 99
      const cb = CATEGORY_SORT[b.metadata.category] ?? 99
      if (ca !== cb) return ca - cb
      const sa = a.metadata.sort_order ?? 0
      const sb = b.metadata.sort_order ?? 0
      if (sa !== sb) return sa - sb
      return a.metadata.id.localeCompare(b.metadata.id)
    })
  }, [experts])

  const filteredExperts = useMemo(() => {
    const q = searchQuery.trim().toLowerCase()
    if (!q) return sortedExperts
    return sortedExperts.filter((item) => {
      const name = pickLocalized(item.metadata.display_name, locale)
      const desc = pickLocalized(item.metadata.description, locale)
      return (
        item.metadata.id.toLowerCase().includes(q) ||
        name.toLowerCase().includes(q) ||
        desc.toLowerCase().includes(q)
      )
    })
  }, [sortedExperts, searchQuery, locale])

  const groupedExperts = useMemo(() => {
    const groups = new Map<string, ExpertListItem[]>()
    for (const item of filteredExperts) {
      const key = item.metadata.category
      const list = groups.get(key) ?? []
      list.push(item)
      groups.set(key, list)
    }
    return Array.from(groups.entries()).sort(
      (a, b) => (CATEGORY_SORT[a[0]] ?? 99) - (CATEGORY_SORT[b[0]] ?? 99)
    )
  }, [filteredExperts])

  const selectedExpert = useMemo(
    () => experts.find((e) => e.metadata.id === selectedExpertId) ?? null,
    [experts, selectedExpertId]
  )

  // Auto-select first expert once loaded.
  useEffect(() => {
    if (!selectedExpertId && sortedExperts.length > 0) {
      setSelectedExpertId(sortedExperts[0].metadata.id)
    }
  }, [selectedExpertId, sortedExperts])

  // Load content + status for the currently selected expert.
  useEffect(() => {
    if (!selectedExpert) {
      setContent("")
      setStatuses({})
      return
    }
    const expertId = selectedExpert.metadata.id
    let cancelled = false
    setContentLoading(true)
    setStatusLoading(true)
    Promise.all([
      expertsReadContent(expertId),
      expertsGetInstallStatus(expertId),
    ])
      .then(([body, statusList]) => {
        if (cancelled) return
        setContent(body)
        const map: Record<string, ExpertInstallStatus> = {}
        for (const entry of statusList) {
          map[entry.agentType] = entry
        }
        setStatuses(map)
      })
      .catch((err) => {
        if (cancelled) return
        const message = toErrorMessage(err)
        toast.error(t("toasts.loadFailed"), { description: message })
      })
      .finally(() => {
        if (!cancelled) {
          setContentLoading(false)
          setStatusLoading(false)
        }
      })
    return () => {
      cancelled = true
    }
  }, [selectedExpert, t])

  const handleToggle = useCallback(
    async (expertId: string, agentType: AgentType, enable: boolean) => {
      const key = `${expertId}:${agentType}`
      setPendingMutation(key)
      try {
        if (enable) {
          const next = await expertsLinkToAgent({ expertId, agentType })
          setStatuses((prev) => ({ ...prev, [agentType]: next }))
          invalidateAgentExpertsCache(agentType)
          toast.success(t("toasts.enabled"))
        } else {
          await expertsUnlinkFromAgent({ expertId, agentType })
          // Re-fetch status to get the accurate post-unlink state.
          const latest = await expertsGetInstallStatus(expertId)
          const map: Record<string, ExpertInstallStatus> = {}
          for (const entry of latest) {
            map[entry.agentType] = entry
          }
          setStatuses(map)
          invalidateAgentExpertsCache(agentType)
          toast.success(t("toasts.disabled"))
        }
      } catch (err) {
        const message = toErrorMessage(err)
        toast.error(
          enable ? t("toasts.enableFailed") : t("toasts.disableFailed"),
          {
            description: message,
          }
        )
      } finally {
        setPendingMutation(null)
      }
    },
    [t]
  )

  const handleOpenCentralDir = useCallback(async () => {
    try {
      const path = await expertsOpenCentralDir()
      if (isDesktop() && getActiveRemoteConnectionId() === null) {
        // Desktop: reveal the central skills folder in Finder / File Explorer.
        // `revealItemInDir` (not `openPath`) is used deliberately: the opener
        // plugin's path scope (`$HOME/**`) defaults to require-literal-leading-
        // dot on Unix, so `openPath` is rejected for the hidden `~/.codeg/...`
        // path. `revealItemInDir` is not scope-checked, mirroring the file tree.
        await revealItemInDir(path)
      } else {
        // Web / remote desktop: no local file manager, so fall back to
        // opening it as an in-app workspace folder.
        await openFolder(path)
      }
    } catch (err) {
      const message = toErrorMessage(err)
      toast.error(t("toasts.openFolderFailed"), { description: message })
    }
  }, [t])

  const safeContainerWidth =
    panelContainerWidth > 0 ? panelContainerWidth : 1200
  const leftMinSize = clamp(
    toPercent(LEFT_MIN_WIDTH, safeContainerWidth),
    5,
    95
  )
  const rightMinSize = clamp(
    toPercent(RIGHT_MIN_WIDTH, safeContainerWidth),
    5,
    95
  )
  const leftMaxSize = Math.max(leftMinSize, 100 - rightMinSize)

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  const selectedName = selectedExpert
    ? pickLocalized(selectedExpert.metadata.display_name, locale) ||
      selectedExpert.metadata.id
    : ""
  const selectedDescription = selectedExpert
    ? pickLocalized(selectedExpert.metadata.description, locale)
    : ""
  const selectedIcon = getIcon(selectedExpert?.metadata.icon ?? null)
  const SelectedIcon = selectedIcon

  return (
    <div className="h-full flex flex-col p-3 md:p-4">
      <div className="flex items-center justify-between gap-3 pb-4">
        <div>
          <h2 className="text-base font-semibold">{t("title")}</h2>
          <p className="text-xs text-muted-foreground mt-1">
            {t("description")}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              handleOpenCentralDir().catch((err) => {
                console.error("[ExpertsSettings] open central dir failed:", err)
              })
            }}
          >
            <FolderOpen className="h-3.5 w-3.5" />
            {t("actions.openCentralDir")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              refresh().catch((err) => {
                console.error("[ExpertsSettings] refresh failed:", err)
              })
            }}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("actions.refresh")}
          </Button>
        </div>
      </div>

      {loadError && (
        <div className="mb-3 rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
          {loadError}
        </div>
      )}

      {experts.length === 0 ? (
        <div className="h-full rounded-lg border bg-card flex items-center justify-center text-sm text-muted-foreground">
          {t("emptyExperts")}
        </div>
      ) : (
        <div ref={panelContainerRef} className="flex-1 min-h-0 min-w-0">
          <ResizablePanelGroup
            direction="horizontal"
            className="h-full min-h-0 min-w-0"
          >
            <ResizablePanel
              defaultSize={38}
              minSize={leftMinSize}
              maxSize={leftMaxSize}
            >
              <div className="min-h-0 h-full min-w-0 rounded-lg border bg-card flex flex-col overflow-hidden lg:rounded-r-none">
                <div className="border-b p-3 space-y-2.5">
                  <Input
                    value={searchQuery}
                    onChange={(event) => setSearchQuery(event.target.value)}
                    placeholder={t("searchPlaceholder")}
                  />
                </div>
                <div className="flex-1 min-h-0 overflow-y-auto p-2 space-y-3">
                  {groupedExperts.map(([category, items]) => (
                    <div key={category} className="space-y-1.5">
                      <div className="px-1 text-[11px] uppercase tracking-wide font-semibold text-muted-foreground">
                        {translatedCategory(category)}
                      </div>
                      {items.map((item) => {
                        const Icon = getIcon(item.metadata.icon)
                        const name =
                          pickLocalized(item.metadata.display_name, locale) ||
                          item.metadata.id
                        const desc = pickLocalized(
                          item.metadata.description,
                          locale
                        )
                        const isActive = selectedExpertId === item.metadata.id
                        return (
                          <button
                            key={item.metadata.id}
                            type="button"
                            onClick={() =>
                              setSelectedExpertId(item.metadata.id)
                            }
                            className={cn(
                              "w-full rounded-md border px-2.5 py-2 text-left transition-colors",
                              isActive
                                ? "border-primary/60 bg-primary/5"
                                : "hover:bg-muted/30"
                            )}
                          >
                            <div className="flex items-start gap-2 min-w-0">
                              <Icon className="h-4 w-4 mt-0.5 shrink-0 text-primary/80" />
                              <div className="min-w-0 flex-1">
                                <div className="text-sm font-medium truncate">
                                  {name}
                                </div>
                                <div className="text-[11px] text-muted-foreground line-clamp-2 mt-0.5">
                                  {desc}
                                </div>
                              </div>
                              {item.user_modified && (
                                <Badge
                                  variant="outline"
                                  className="h-5 px-1.5 text-[10px] shrink-0 border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400"
                                >
                                  {t("badges.userModified")}
                                </Badge>
                              )}
                            </div>
                          </button>
                        )
                      })}
                    </div>
                  ))}
                  {groupedExperts.length === 0 && (
                    <div className="text-xs text-muted-foreground px-2 py-3">
                      {t("emptySearch")}
                    </div>
                  )}
                </div>
              </div>
            </ResizablePanel>

            <ResizableHandle withHandle />

            <ResizablePanel defaultSize={62} minSize={rightMinSize}>
              <div className="h-full flex-1 min-h-0 min-w-0 rounded-lg border bg-card overflow-hidden lg:rounded-l-none lg:border-l-0">
                {selectedExpert ? (
                  <div className="h-full flex flex-col">
                    <div className="border-b px-4 py-3 flex items-start gap-3">
                      <SelectedIcon className="h-5 w-5 mt-0.5 shrink-0 text-primary/80" />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <h3 className="text-sm font-semibold truncate">
                            {selectedName}
                          </h3>
                          <Badge
                            variant="outline"
                            className="h-5 px-1.5 text-[10px]"
                          >
                            {translatedCategory(
                              selectedExpert.metadata.category
                            )}
                          </Badge>
                          <code className="text-[11px] text-muted-foreground font-mono truncate">
                            {selectedExpert.metadata.id}
                          </code>
                        </div>
                        <p className="text-xs text-muted-foreground mt-1">
                          {selectedDescription}
                        </p>
                      </div>
                    </div>

                    <div className="flex-1 overflow-y-auto p-4 space-y-4">
                      <div className="rounded-md border p-3">
                        <div className="text-[11px] text-muted-foreground mb-2 flex items-center justify-between">
                          <span>{t("enableForAgents")}</span>
                          {statusLoading && (
                            <Loader2 className="h-3 w-3 animate-spin" />
                          )}
                        </div>
                        <div className="space-y-1.5">
                          {agents.length === 0 ? (
                            <div className="text-xs text-muted-foreground py-2">
                              {t("noAgents")}
                            </div>
                          ) : (
                            agents.map((agent) => {
                              const status = statuses[agent.agent_type] ?? null
                              const enabled =
                                status?.state === "linked_to_codeg"
                              const blocked =
                                status?.state === "blocked_by_real_directory" ||
                                status?.state === "linked_elsewhere"
                              const key = `${selectedExpert.metadata.id}:${agent.agent_type}`
                              const pending = pendingMutation === key
                              return (
                                <div
                                  key={agent.agent_type}
                                  className={cn(
                                    "flex items-center gap-3 rounded-md border px-3 py-2",
                                    enabled
                                      ? "border-primary/40 bg-primary/5"
                                      : "border-border"
                                  )}
                                >
                                  <div className="flex-1 min-w-0">
                                    <div className="text-sm font-medium truncate">
                                      {agent.name}
                                    </div>
                                    <div className="text-[11px] text-muted-foreground truncate">
                                      {status
                                        ? translatedState(status.state)
                                        : "—"}
                                    </div>
                                    {status?.copyMode && (
                                      <div className="text-[11px] text-amber-500 mt-0.5">
                                        {t("copyModeWarning")}
                                      </div>
                                    )}
                                  </div>
                                  <Switch
                                    checked={enabled}
                                    disabled={pending || (blocked && !enabled)}
                                    onCheckedChange={(checked: boolean) => {
                                      handleToggle(
                                        selectedExpert.metadata.id,
                                        agent.agent_type,
                                        checked
                                      ).catch((err) => {
                                        console.error(
                                          "[ExpertsSettings] toggle failed:",
                                          err
                                        )
                                      })
                                    }}
                                  />
                                </div>
                              )
                            })
                          )}
                        </div>
                      </div>

                      <div className="rounded-md border p-3">
                        <div className="text-[11px] text-muted-foreground mb-2">
                          {t("previewTitle")}
                        </div>
                        {contentLoading ? (
                          <div className="flex items-center gap-2 text-xs text-muted-foreground py-3">
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("loadingContent")}
                          </div>
                        ) : (
                          <div
                            className={cn(
                              "text-sm leading-6 rounded-md bg-muted/10 p-3 overflow-auto max-h-[480px]",
                              "[&_h1]:text-xl [&_h1]:font-semibold [&_h1]:mb-3",
                              "[&_h2]:text-lg [&_h2]:font-semibold [&_h2]:mt-5 [&_h2]:mb-2",
                              "[&_h3]:text-base [&_h3]:font-semibold [&_h3]:mt-4 [&_h3]:mb-2",
                              "[&_p]:mb-3 [&_li]:mb-1",
                              "[&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5",
                              "[&_code]:font-mono [&_code]:text-xs [&_code]:bg-muted [&_code]:rounded [&_code]:px-1",
                              "[&_pre]:bg-muted [&_pre]:rounded-md [&_pre]:p-3 [&_pre]:overflow-x-auto"
                            )}
                          >
                            <ReactMarkdown remarkPlugins={[remarkGfm]}>
                              {stripFrontmatter(content)}
                            </ReactMarkdown>
                          </div>
                        )}
                      </div>
                    </div>
                  </div>
                ) : (
                  <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
                    {t("emptySelection")}
                  </div>
                )}
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </div>
      )}
    </div>
  )
}
