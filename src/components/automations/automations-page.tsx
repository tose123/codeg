"use client"

import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  CalendarClock,
  Clock,
  CirclePlay,
  Folder,
  GitBranch,
  ListFilter,
  Loader2,
  MoreHorizontal,
  MousePointerClick,
  Pencil,
  Play,
  Plus,
  Power,
  PowerOff,
  RotateCw,
  SlidersHorizontal,
  SquareArrowOutUpRight,
  Trash2,
  X,
  Zap,
} from "lucide-react"
import { useAutomationsView } from "@/contexts/automations-view-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useTabActions } from "@/contexts/tab-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { AutomationEditor } from "./automation-editor"
import {
  templateToDraft,
  type AutomationTemplate,
} from "./automation-templates"
import { TemplateGallery } from "./template-gallery"
import { ScheduleLabel } from "./schedule-label"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { AgentIcon } from "@/components/agent-icon"
import { Button } from "@/components/ui/button"
import { Badge } from "@/components/ui/badge"
import { Switch } from "@/components/ui/switch"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import {
  automationCancelRun,
  automationCreate,
  automationDelete,
  automationMarkSeen,
  automationRunNow,
  automationRuns,
  automationSetEnabled,
  automationUpdate,
} from "@/lib/api"
import { onTransportReconnect, subscribe } from "@/lib/platform"
import { cn } from "@/lib/utils"
import type { Automation, AutomationDraft, AutomationRun } from "@/lib/types"

const AUTOMATION_CHANGED_EVENT = "automation://changed"

const STATUS_STYLES: Record<string, string> = {
  running: "bg-primary/10 text-primary",
  succeeded: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  failed: "bg-destructive/10 text-destructive",
  cancelled: "bg-muted text-muted-foreground",
  skipped: "bg-muted text-muted-foreground",
}

function StatusChip({ status }: { status: string | null }) {
  const t = useTranslations("Automations")
  if (!status) return null
  const label =
    {
      running: t("statusRunning"),
      succeeded: t("statusSucceeded"),
      failed: t("statusFailed"),
      cancelled: t("statusCancelled"),
      skipped: t("statusSkipped"),
    }[status] ?? status
  return (
    <span
      className={cn(
        "inline-flex h-5 shrink-0 items-center rounded-full px-2 text-[0.6875rem] font-medium",
        STATUS_STYLES[status] ?? "bg-muted text-muted-foreground"
      )}
    >
      {label}
    </span>
  )
}

// Compact, i18n-free relative time ("now"/"5m"/"2h"/"3d"/"2mo"/"1y"), matching
// the sidebar conversation list's style. Absolute time rides in the title attr.
function formatRelative(iso: string | null, now: number): string {
  if (!iso) return "—"
  const ts = Date.parse(iso)
  if (Number.isNaN(ts)) return "—"
  const sec = Math.max(0, Math.round((now - ts) / 1000))
  if (sec < 45) return "now"
  const min = Math.round(sec / 60)
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const day = Math.round(hr / 24)
  if (day < 30) return `${day}d`
  const mo = Math.round(day / 30)
  if (mo < 12) return `${mo}mo`
  return `${Math.round(mo / 12)}y`
}

// Forward-looking sibling of formatRelative ("1m"/"3h"/"2d") for the next run.
// Floors at 1m so an imminent run never renders as "0m".
function formatRelativeFuture(iso: string | null, now: number): string {
  if (!iso) return "—"
  const ts = Date.parse(iso)
  if (Number.isNaN(ts)) return "—"
  const sec = Math.max(0, Math.round((ts - now) / 1000))
  const min = Math.max(1, Math.round(sec / 60))
  if (min < 60) return `${min}m`
  const hr = Math.round(min / 60)
  if (hr < 24) return `${hr}h`
  const day = Math.round(hr / 24)
  if (day < 30) return `${day}d`
  const mo = Math.round(day / 30)
  if (mo < 12) return `${mo}mo`
  return `${Math.round(mo / 12)}y`
}

function formatDuration(
  startIso: string | null,
  endIso: string | null
): string {
  if (!startIso || !endIso) return "—"
  const start = Date.parse(startIso)
  const end = Date.parse(endIso)
  if (Number.isNaN(start) || Number.isNaN(end) || end < start) return "—"
  const sec = Math.round((end - start) / 1000)
  if (sec < 60) return `${sec}s`
  const min = Math.floor(sec / 60)
  const rem = sec % 60
  if (min < 60) return rem ? `${min}m ${rem}s` : `${min}m`
  const hr = Math.floor(min / 60)
  return `${hr}h ${min % 60}m`
}

// Absolute local date-time for run-history rows; null/invalid → "—".
function formatDateTime(iso: string | null): string {
  if (!iso) return "—"
  const ts = Date.parse(iso)
  if (Number.isNaN(ts)) return "—"
  return new Date(ts).toLocaleString()
}

/** The detail pane's three states. "gallery" is the template picker shown when
 *  starting a new automation; "editor" hosts the form, seeded from a template
 *  (create) or an existing automation (edit). */
type EditingState =
  | { kind: "create"; seed: AutomationDraft | null }
  | { kind: "edit"; automation: Automation }

export function AutomationsPage() {
  const t = useTranslations("Automations")
  const { automations, unseenFailures, refetch } = useAutomationsView()
  const folders = useAppWorkspaceStore((s) => s.folders)
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [mode, setMode] = useState<"detail" | "gallery" | "editor">("detail")
  const [editing, setEditing] = useState<EditingState | null>(null)

  // Clear the unseen-failure badges while the page is open — on entry and again
  // whenever a new failure arrives live (the failed run is already on screen, so
  // the sidebar badge shouldn't keep nagging). Keying on unseenFailures rather
  // than mount makes it re-fire on the automation://changed refetch; it
  // converges because markSeen drives the count to 0, after which this early
  // returns. refetch is stable.
  useEffect(() => {
    if (unseenFailures === 0) return
    void automationMarkSeen()
      .then(() => refetch())
      .catch(() => {})
  }, [unseenFailures, refetch])

  const hasAutomations = automations.length > 0
  // The shown automation: the explicit selection, else the first row, so the
  // detail pane is never blank when automations exist. Derived (no effect) so a
  // deleted selection cleanly falls back instead of dangling.
  const current =
    automations.find((a) => a.id === selectedId) ?? automations[0] ?? null
  // Frozen at mount — the page remounts on each route entry, so relative labels
  // ("Next in 3h") are anchored to when Automations was opened. Reading Date.now
  // during render is impure (react-hooks/purity); this is the RunHistory idiom.
  const [now] = useState(() => Date.now())

  // List filters (folder + enabled state), ephemeral per page mount.
  const [folderFilter, setFolderFilter] = useState<number | "all">("all")
  const [statusFilter, setStatusFilter] = useState<
    "all" | "enabled" | "disabled"
  >("all")
  const visibleAutomations = useMemo(
    () =>
      automations.filter(
        (a) =>
          (folderFilter === "all" || a.root_folder_id === folderFilter) &&
          (statusFilter === "all" ||
            (statusFilter === "enabled" ? a.enabled : !a.enabled))
      ),
    [automations, folderFilter, statusFilter]
  )

  const openGallery = () => {
    setEditing(null)
    setMode("gallery")
  }
  const backToGallery = () => {
    setEditing(null)
    setMode("gallery")
  }
  const closeToDetail = () => {
    setEditing(null)
    setMode("detail")
  }
  const pickTemplate = (tpl: AutomationTemplate | null) => {
    const seed = tpl
      ? templateToDraft(tpl, {
          name: t(tpl.titleKey),
          agentType: "claude_code",
          folderId: folders[0]?.id ?? null,
        })
      : null
    setEditing({ kind: "create", seed })
    setMode("editor")
  }
  const startEdit = (a: Automation) => {
    setEditing({ kind: "edit", automation: a })
    setMode("editor")
  }
  const selectAutomation = (a: Automation) => {
    setSelectedId(a.id)
    setEditing(null)
    setMode("detail")
  }

  // Shared mutation runner for the per-row quick actions (run now / toggle /
  // delete) hoisted out of the detail pane so the list's ⋯ menu can drive them.
  const runAction = useCallback(
    async (fn: () => Promise<unknown>) => {
      try {
        await fn()
        await refetch()
      } catch (e) {
        toast.error(e instanceof Error ? e.message : String(e))
      }
    },
    [refetch]
  )

  const handleSubmit = async (draft: AutomationDraft) => {
    const saved =
      editing?.kind === "edit"
        ? await automationUpdate(editing.automation.id, draft)
        : await automationCreate(draft)
    await refetch()
    setSelectedId(saved.id)
    closeToDetail()
  }

  const editorPane =
    editing != null ? (
      <ScrollArea className="h-full">
        <div className="mx-auto w-full max-w-2xl p-4 sm:p-6">
          <AutomationEditor
            // Key by edit target so switching to a different automation (e.g.
            // ⋯ → Edit on another row while the editor is open) remounts with
            // fresh state instead of showing the previous target's fields.
            key={
              editing.kind === "edit"
                ? `edit-${editing.automation.id}`
                : "create"
            }
            automation={
              editing.kind === "edit" ? editing.automation : editing.seed
            }
            onSubmit={handleSubmit}
            onCancel={closeToDetail}
            onBackToTemplates={
              editing.kind === "create" ? backToGallery : undefined
            }
          />
        </div>
      </ScrollArea>
    ) : null

  const picker = (onboarding: boolean) => (
    <ScrollArea className="h-full">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-6 p-4 sm:p-6">
        {onboarding ? (
          <div className="flex flex-col items-center gap-2 pt-4 text-center">
            <span className="flex size-12 items-center justify-center rounded-2xl bg-muted text-muted-foreground">
              <Zap className="size-6" aria-hidden="true" />
            </span>
            <h2 className="text-base font-semibold">{t("onboardTitle")}</h2>
            <p className="max-w-md text-sm text-muted-foreground">
              {t("onboardHint")}
            </p>
          </div>
        ) : (
          <div className="flex items-center justify-between gap-2">
            <h2 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
              {t("startFromTemplate")}
            </h2>
            <Button size="sm" variant="ghost" onClick={closeToDetail}>
              {t("cancel")}
            </Button>
          </div>
        )}
        <TemplateGallery onPick={pickTemplate} />
      </div>
    </ScrollArea>
  )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {hasAutomations ? (
        <ResizablePanelGroup direction="horizontal" className="min-h-0 flex-1">
          <ResizablePanel
            id="automations-list"
            order={1}
            defaultSize={32}
            minSize={22}
          >
            <div className="@container flex h-full flex-col">
              <PageHeader showNew={mode === "detail"} onNew={openGallery} />
              {automations.length > 1 ? (
                <ListFilters
                  folders={folders}
                  folderFilter={folderFilter}
                  onFolderFilter={setFolderFilter}
                  statusFilter={statusFilter}
                  onStatusFilter={setStatusFilter}
                />
              ) : null}
              <ScrollArea className="min-h-0 flex-1">
                {visibleAutomations.length === 0 ? (
                  <p className="px-3 py-6 text-center text-xs text-muted-foreground">
                    {t("noMatches")}
                  </p>
                ) : (
                  <ul className="flex flex-col gap-0.5 p-1.5">
                    {visibleAutomations.map((a) => (
                      <AutomationListItem
                        key={a.id}
                        automation={a}
                        now={now}
                        selected={mode === "detail" && current?.id === a.id}
                        onSelect={() => selectAutomation(a)}
                        onRunNow={() => runAction(() => automationRunNow(a.id))}
                        onToggleEnabled={() =>
                          runAction(() =>
                            automationSetEnabled(a.id, !a.enabled)
                          )
                        }
                        onEdit={() => startEdit(a)}
                        onDelete={() => runAction(() => automationDelete(a.id))}
                      />
                    ))}
                  </ul>
                )}
              </ScrollArea>
            </div>
          </ResizablePanel>
          <ResizableHandle withHandle />
          <ResizablePanel id="automations-detail" order={2} defaultSize={68}>
            {mode === "editor" && editing ? (
              editorPane
            ) : mode === "gallery" ? (
              picker(false)
            ) : current ? (
              <AutomationDetail
                automation={current}
                refetch={refetch}
                onEdit={() => startEdit(current)}
              />
            ) : (
              // Defensive only: `current` falls back to automations[0], which is
              // always present inside this hasAutomations branch, so this arm is
              // not reached in practice.
              <div className="flex h-full items-center justify-center p-8 text-center text-xs text-muted-foreground">
                {t("selectHint")}
              </div>
            )}
          </ResizablePanel>
        </ResizablePanelGroup>
      ) : (
        <div className="flex h-full min-h-0 flex-col">
          <PageHeader showNew={false} onNew={openGallery} />
          <div className="min-h-0 flex-1">
            {mode === "editor" && editing ? editorPane : picker(true)}
          </div>
        </div>
      )}
    </div>
  )
}

// Folder + enabled-state filters above the list. The folder select only appears
// when the workspace has more than one folder; the status select is always shown.
function ListFilters({
  folders,
  folderFilter,
  onFolderFilter,
  statusFilter,
  onStatusFilter,
}: {
  folders: Array<{ id: number; name: string }>
  folderFilter: number | "all"
  onFolderFilter: (v: number | "all") => void
  statusFilter: "all" | "enabled" | "disabled"
  onStatusFilter: (v: "all" | "enabled" | "disabled") => void
}) {
  const t = useTranslations("Automations")
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-border px-2 py-1.5">
      <Select
        value={statusFilter}
        onValueChange={(v) =>
          onStatusFilter(v as "all" | "enabled" | "disabled")
        }
      >
        <SelectTrigger size="sm" className="h-7 w-auto gap-1.5 text-xs">
          <ListFilter
            className="size-3.5 text-muted-foreground"
            aria-hidden="true"
          />
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">{t("filterAll")}</SelectItem>
          <SelectItem value="enabled">{t("enabled")}</SelectItem>
          <SelectItem value="disabled">{t("statusDisabled")}</SelectItem>
        </SelectContent>
      </Select>
      {folders.length > 1 ? (
        <Select
          value={folderFilter === "all" ? "all" : String(folderFilter)}
          onValueChange={(v) => onFolderFilter(v === "all" ? "all" : Number(v))}
        >
          <SelectTrigger
            size="sm"
            className="h-7 w-auto max-w-[12rem] gap-1.5 text-xs"
          >
            <Folder
              className="size-3.5 text-muted-foreground"
              aria-hidden="true"
            />
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("allFolders")}</SelectItem>
            {folders.map((f) => (
              <SelectItem key={f.id} value={String(f.id)}>
                {f.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}
    </div>
  )
}

function PageHeader({
  showNew,
  onNew,
}: {
  showNew: boolean
  onNew: () => void
}) {
  const t = useTranslations("Automations")
  return (
    <header className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border pl-3.5 pr-2.5">
      <div className="flex min-w-0 items-center gap-2">
        <Zap
          className="size-4 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
        <h1 className="truncate text-sm font-semibold">{t("title")}</h1>
      </div>
      {showNew ? (
        <Button
          size="sm"
          onClick={onNew}
          aria-label={t("new")}
          title={t("new")}
        >
          <Plus className="h-3.5 w-3.5" aria-hidden="true" />
          {/* Collapses to a "+"-only button when the pane is too narrow for both
              the title and the labeled button (the @container is the list pane). */}
          <span className="hidden @[16rem]:inline">{t("new")}</span>
        </Button>
      ) : null}
    </header>
  )
}

// A single status dot riding the agent icon, mirroring the sidebar conversation
// row. It blends two facts: a disabled automation reads muted regardless of
// history; an enabled one is colored by its last run (emerald when it has never
// run yet, i.e. "ready").
const RUN_STATUS_DOT: Record<string, string> = {
  running: "bg-amber-500",
  succeeded: "bg-emerald-500",
  failed: "bg-destructive",
  cancelled: "bg-muted-foreground/50",
  skipped: "bg-muted-foreground/50",
}

// Run-history timeline node tint per status — colors the node ring and the
// trigger icon inside it; falls back to a neutral border for unknown states.
const RUN_NODE_RING: Record<string, string> = {
  running: "border-amber-500/50 text-amber-600 dark:text-amber-400",
  succeeded: "border-emerald-500/50 text-emerald-600 dark:text-emerald-400",
  failed: "border-destructive/50 text-destructive",
  cancelled: "border-border text-muted-foreground",
  skipped: "border-border text-muted-foreground",
}

function AutomationDot({
  enabled,
  status,
}: {
  enabled: boolean
  status: string | null
}) {
  const color = !enabled
    ? "bg-muted-foreground/40"
    : status
      ? (RUN_STATUS_DOT[status] ?? "bg-emerald-500")
      : "bg-emerald-500"
  return (
    <span
      className={cn(
        "block size-1.5 rounded-full ring-2 ring-background",
        color
      )}
      aria-hidden="true"
    />
  )
}

function AutomationListItem({
  automation,
  now,
  selected,
  onSelect,
  onRunNow,
  onToggleEnabled,
  onEdit,
  onDelete,
}: {
  automation: Automation
  now: number
  selected: boolean
  onSelect: () => void
  onRunNow: () => void
  onToggleEnabled: () => void
  onEdit: () => void
  onDelete: () => void
}) {
  const t = useTranslations("Automations")
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)
  const isSchedule = automation.trigger_kind === "schedule" && !!automation.cron
  const isRunning = automation.last_run_status === "running"
  const showNextIn =
    isSchedule && automation.enabled && !!automation.next_run_at
  const timeLabel = showNextIn
    ? t("nextIn", { rel: formatRelativeFuture(automation.next_run_at, now) })
    : automation.last_run_at
      ? formatRelative(automation.last_run_at, now)
      : null

  // The row's quick actions, authored once so the ⋯ dropdown and the right-click
  // context menu render exactly the same set (the user asked for parity).
  const actions: Array<{
    key: string
    icon: React.ReactNode
    label: string
    onSelect: () => void
    variant?: "destructive"
    separatorBefore?: boolean
  }> = [
    {
      key: "run",
      icon: <Play className="size-3.5" aria-hidden="true" />,
      label: t("runNow"),
      onSelect: onRunNow,
    },
    {
      key: "toggle",
      icon: automation.enabled ? (
        <PowerOff className="size-3.5" aria-hidden="true" />
      ) : (
        <Power className="size-3.5" aria-hidden="true" />
      ),
      label: automation.enabled ? t("disable") : t("enable"),
      onSelect: onToggleEnabled,
    },
    {
      key: "edit",
      icon: <Pencil className="size-3.5" aria-hidden="true" />,
      label: t("edit"),
      onSelect: onEdit,
    },
    {
      key: "delete",
      icon: <Trash2 className="size-3.5" aria-hidden="true" />,
      label: t("delete"),
      // Let the menu close (and restore focus) before the dialog mounts —
      // opening synchronously races focus restoration and self-dismisses.
      onSelect: () => setTimeout(() => setConfirmOpen(true), 0),
      variant: "destructive",
      separatorBefore: true,
    },
  ]

  // Render the shared actions into either menu's item/separator components.
  const renderActions = (
    Item: React.ElementType,
    Separator: React.ElementType
  ) =>
    actions.map((a) => (
      <Fragment key={a.key}>
        {a.separatorBefore ? <Separator /> : null}
        <Item variant={a.variant} onSelect={a.onSelect}>
          {a.icon}
          {a.label}
        </Item>
      </Fragment>
    ))

  return (
    <li>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            className={cn(
              "group flex h-8 w-full items-center rounded-full pr-1 transition-colors",
              selected ? "bg-accent" : "hover:bg-accent/60"
            )}
          >
            <button
              type="button"
              onClick={onSelect}
              className="flex h-full min-w-0 flex-1 items-center gap-2.5 rounded-full pl-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
            >
              <span className="relative flex size-5 shrink-0 items-center justify-center">
                <AgentIcon
                  agentType={automation.agent_type}
                  className="size-4"
                />
                <span className="absolute -right-0.5 -bottom-0.5">
                  <AutomationDot
                    enabled={automation.enabled}
                    status={automation.last_run_status}
                  />
                </span>
              </span>
              <span
                className={cn(
                  "min-w-0 flex-1 truncate text-sm",
                  automation.enabled
                    ? "font-medium"
                    : "font-normal text-muted-foreground"
                )}
              >
                {automation.name}
              </span>
            </button>

            <div className="flex shrink-0 items-center gap-0.5 pl-1">
              {/* Time yields to the ⋯ affordance on hover, keyboard focus, or
                  while the menu is open — mirroring the conversation row. */}
              <span
                className={cn(
                  "flex items-center group-hover:hidden group-focus-within:hidden",
                  menuOpen && "hidden"
                )}
              >
                {isRunning ? (
                  <Loader2
                    className="size-3.5 animate-spin text-amber-600 dark:text-amber-400"
                    aria-hidden="true"
                  />
                ) : timeLabel ? (
                  <span
                    className={cn(
                      "shrink-0 tabular-nums text-[0.71875rem]",
                      selected
                        ? "font-medium text-muted-foreground"
                        : "text-muted-foreground/70"
                    )}
                  >
                    {timeLabel}
                  </span>
                ) : null}
              </span>

              <DropdownMenu onOpenChange={setMenuOpen}>
                <DropdownMenuTrigger asChild>
                  {/* Hidden when idle (the time sits in its place); reveals on
                      hover, on keyboard focus entering the row, and while open.
                      justify-end flushes the glyph to the time's right edge;
                      no hover/open fill — only the icon color shifts. */}
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="hidden justify-end text-muted-foreground/80 hover:bg-transparent hover:text-foreground group-hover:flex group-focus-within:flex aria-expanded:bg-transparent data-[state=open]:flex dark:hover:bg-transparent"
                    aria-label={t("moreActions")}
                  >
                    <MoreHorizontal className="size-4" aria-hidden="true" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-40">
                  {renderActions(DropdownMenuItem, DropdownMenuSeparator)}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </div>
        </ContextMenuTrigger>
        {/* Right-click anywhere on the row opens the same actions as ⋯. */}
        <ContextMenuContent className="w-40">
          {renderActions(ContextMenuItem, ContextMenuSeparator)}
        </ContextMenuContent>
      </ContextMenu>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("deleteDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={onDelete}>
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  )
}

// One fact per card — icon + uppercase label on top, value below. Replaces the
// old dense InfoRow grid so the detail's "Schedule & target" reads as a tidy set
// of stat cards rather than a cramped two-column list.
function StatCard({
  icon,
  label,
  children,
  className,
}: {
  icon: React.ReactNode
  label: string
  children: React.ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-xl border border-border bg-card p-3",
        className
      )}
    >
      <div className="flex items-center gap-1.5 text-muted-foreground [&>svg]:size-3.5">
        {icon}
        <span className="text-[0.6875rem] font-medium uppercase tracking-wide">
          {label}
        </span>
      </div>
      <div className="min-w-0 text-sm">{children}</div>
    </div>
  )
}

function SectionCard({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <section className="rounded-xl border border-border bg-card p-4">
      <h3 className="mb-3 text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      {children}
    </section>
  )
}

function AutomationDetail({
  automation,
  refetch,
  onEdit,
}: {
  automation: Automation
  refetch: () => Promise<void>
  onEdit: () => void
}) {
  const t = useTranslations("Automations")
  const folders = useAppWorkspaceStore((s) => s.folders)
  const [busy, setBusy] = useState(false)

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
      await refetch()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const folderName =
    folders.find((f) => f.id === automation.root_folder_id)?.name ?? "—"
  // `config` is serialized from an opaque JSON column and falls back to `null`
  // on the backend if the stored blob fails to parse — guard every read so a
  // malformed automation degrades gracefully instead of throwing during render
  // (which, with no error boundary on this route, white-screens the whole app).
  const config = automation.config ?? null
  const labels = config?.label_snapshot
  const configEntries = Object.entries(config?.config_values ?? {})
  const isSchedule = automation.trigger_kind === "schedule" && !!automation.cron

  return (
    <ScrollArea className="h-full">
      <div className="@container mx-auto flex w-full max-w-3xl flex-col gap-4 p-4 sm:p-6">
        <div className="flex items-start justify-between gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex min-w-0 items-center gap-2">
              <h2 className="truncate text-lg font-semibold">
                {automation.name}
              </h2>
              <StatusChip status={automation.last_run_status} />
            </div>
          </div>
          <label className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
            {automation.enabled ? t("enabled") : t("statusDisabled")}
            <Switch
              checked={automation.enabled}
              disabled={busy}
              onCheckedChange={(v) =>
                run(() => automationSetEnabled(automation.id, v))
              }
              aria-label={t("enabled")}
            />
          </label>
        </div>

        <div className="flex flex-col gap-2">
          <h3 className="text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
            {t("sectionSchedule")}
          </h3>
          <div className="grid grid-cols-1 gap-3 @sm:grid-cols-2 @xl:grid-cols-3">
            <StatCard
              icon={isSchedule ? <CalendarClock /> : <MousePointerClick />}
              label={t("trigger")}
            >
              {isSchedule && automation.cron ? (
                <span className="flex flex-col gap-0.5">
                  <ScheduleLabel cron={automation.cron} />
                  <span className="font-mono text-xs text-muted-foreground">
                    {automation.cron}
                  </span>
                </span>
              ) : (
                t("manual")
              )}
            </StatCard>
            <StatCard
              icon={
                <AgentIcon
                  agentType={automation.agent_type}
                  className="size-3.5"
                />
              }
              label={t("agent")}
            >
              <span className="block truncate">
                {labels?.agent_label ?? automation.agent_type}
              </span>
            </StatCard>
            <StatCard icon={<Folder />} label={t("folder")}>
              <span className="block truncate">
                {labels?.folder_label ?? folderName}
              </span>
            </StatCard>
            <StatCard icon={<GitBranch />} label={t("isolation")}>
              <span className="block">
                {automation.isolation === "worktree_per_run"
                  ? t("isolationWorktree")
                  : t("isolationShared")}
                {automation.isolation === "shared_in_root" &&
                automation.branch ? (
                  <span className="ml-1 font-mono text-xs text-muted-foreground">
                    {automation.branch}
                  </span>
                ) : null}
              </span>
            </StatCard>
            {isSchedule ? (
              <StatCard icon={<Clock />} label={t("nextRun")}>
                {automation.next_run_at
                  ? new Date(automation.next_run_at).toLocaleString()
                  : "—"}
              </StatCard>
            ) : null}
            {config?.mode_id || configEntries.length > 0 ? (
              <StatCard icon={<SlidersHorizontal />} label={t("config")}>
                <div className="flex flex-wrap gap-1">
                  {config?.mode_id ? (
                    <Badge variant="outline" className="text-[0.625rem]">
                      {labels?.mode_label ?? config.mode_id}
                    </Badge>
                  ) : null}
                  {configEntries.map(([k, v]) => (
                    <Badge
                      key={k}
                      variant="outline"
                      className="text-[0.625rem]"
                    >
                      {labels?.config_labels?.[k] ?? v}
                    </Badge>
                  ))}
                </div>
              </StatCard>
            ) : null}
          </div>
        </div>

        <SectionCard title={t("sectionPrompt")}>
          <p className="whitespace-pre-wrap text-sm text-foreground/90">
            {config?.display_text || "—"}
          </p>
        </SectionCard>

        <div className="flex gap-2">
          <Button
            onClick={() => run(() => automationRunNow(automation.id))}
            disabled={busy}
          >
            <Play className="size-3.5" aria-hidden="true" />
            {t("runNow")}
          </Button>
          <Button variant="outline" onClick={onEdit}>
            <Pencil className="size-3.5" aria-hidden="true" />
            {t("edit")}
          </Button>
        </div>

        <RunHistory
          key={automation.id}
          automation={automation}
          onChanged={refetch}
        />
      </div>
    </ScrollArea>
  )
}

function RunHistory({
  automation,
  onChanged,
}: {
  automation: Automation
  onChanged: () => Promise<void>
}) {
  const t = useTranslations("Automations")
  const { openTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const [runs, setRuns] = useState<AutomationRun[]>([])
  const [loading, setLoading] = useState(true)
  const reqRef = useRef(0)

  const load = useCallback(async () => {
    const id = ++reqRef.current
    try {
      const list = await automationRuns(automation.id)
      if (id === reqRef.current) {
        setRuns(list)
      }
    } catch {
      // keep the previous list on transient error
    } finally {
      if (id === reqRef.current) setLoading(false)
    }
  }, [automation.id])

  useEffect(() => {
    setLoading(true)
    void load()
    let unsub: (() => void) | undefined
    let cancelled = false
    void subscribe(AUTOMATION_CHANGED_EVENT, () => {
      void load()
    }).then((u: () => void) => {
      if (cancelled) u()
      else unsub = u
    })
    // A run that settled while the WS was disconnected drops its event (the
    // broadcaster skips when receiver_count == 0), so re-load on reconnect to
    // clear a stale "running" row. No-op on desktop IPC.
    const offReconnect = onTransportReconnect(() => {
      void load()
    })
    return () => {
      cancelled = true
      unsub?.()
      offReconnect?.()
    }
  }, [load])

  const viewConversation = (r: AutomationRun) => {
    // Worktree runs live in their own folder; shared runs in the automation's
    // root. Bail rather than open folderId 0 (a structurally broken tab) if
    // neither resolves. openConversations() also covers re-selecting the
    // already-active tab, which wouldn't change activeTabId.
    const folderId = r.worktree_folder_id ?? automation.root_folder_id
    if (r.conversation_id == null || folderId == null) return
    openConversations()
    openTab(folderId, r.conversation_id, automation.agent_type)
  }

  const cancel = async (r: AutomationRun) => {
    try {
      await automationCancelRun(r.id)
      await load()
      await onChanged()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <span className="text-[0.6875rem] font-medium uppercase tracking-wide text-muted-foreground">
          {t("runHistory")}
        </span>
        <Button
          size="icon"
          variant="ghost"
          className="h-6 w-6 text-muted-foreground"
          onClick={() => void load()}
          title={t("refresh")}
          aria-label={t("refresh")}
        >
          <RotateCw className="h-3.5 w-3.5" aria-hidden="true" />
        </Button>
      </div>

      {loading && runs.length === 0 ? (
        <div className="flex items-center gap-2 py-4 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
        </div>
      ) : runs.length === 0 ? (
        <p className="py-4 text-xs text-muted-foreground">{t("noRuns")}</p>
      ) : (
        <ol className="flex flex-col">
          {runs.map((r, i) => (
            <li key={r.id} className="flex gap-3">
              {/* Rail: a status-tinted node + a connector line down to the next
                  run. The connector is omitted on the last item so the line
                  terminates at the final node. */}
              <div className="flex flex-col items-center">
                <span
                  className={cn(
                    "z-10 flex size-7 shrink-0 items-center justify-center rounded-full border-2 bg-background",
                    RUN_NODE_RING[r.status ?? ""] ??
                      "border-border text-muted-foreground"
                  )}
                >
                  {r.trigger === "manual" ? (
                    <CirclePlay className="size-3.5" aria-hidden="true" />
                  ) : (
                    <Clock className="size-3.5" aria-hidden="true" />
                  )}
                </span>
                {i < runs.length - 1 ? (
                  <span className="w-px flex-1 bg-border" aria-hidden="true" />
                ) : null}
              </div>

              <div
                className={cn(
                  "flex min-w-0 flex-1 flex-col gap-0.5",
                  i < runs.length - 1 && "pb-5"
                )}
              >
                <div className="flex items-center gap-2">
                  <StatusChip status={r.status} />
                  {r.conversation_id != null ? (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-6 shrink-0 text-muted-foreground"
                      onClick={() => viewConversation(r)}
                      title={t("viewConversation")}
                      aria-label={t("viewConversation")}
                    >
                      <SquareArrowOutUpRight
                        className="h-3.5 w-3.5"
                        aria-hidden="true"
                      />
                    </Button>
                  ) : null}
                  {r.status === "running" ? (
                    <Button
                      size="icon"
                      variant="ghost"
                      className="size-6 shrink-0 text-muted-foreground hover:text-destructive"
                      onClick={() => void cancel(r)}
                      title={t("cancelRun")}
                      aria-label={t("cancelRun")}
                    >
                      <X className="h-3.5 w-3.5" aria-hidden="true" />
                    </Button>
                  ) : null}
                  <span
                    className="min-w-0 flex-1 truncate text-xs tabular-nums text-muted-foreground"
                    title={
                      r.started_at
                        ? new Date(r.started_at).toLocaleString()
                        : undefined
                    }
                  >
                    {formatDateTime(r.started_at)}
                    {r.ended_at ? (
                      <>
                        {" · "}
                        {formatDuration(r.started_at, r.ended_at)}
                      </>
                    ) : null}
                  </span>
                </div>
                {r.error ? (
                  <span className="truncate text-[0.6875rem] text-destructive">
                    {r.error}
                  </span>
                ) : r.summary ? (
                  <span className="truncate text-[0.6875rem] text-muted-foreground">
                    {r.summary}
                  </span>
                ) : null}
              </div>
            </li>
          ))}
        </ol>
      )}
    </div>
  )
}
