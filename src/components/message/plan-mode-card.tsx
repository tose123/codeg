"use client"

import { memo } from "react"
import { useTranslations } from "next-intl"
import { ListTodoIcon } from "lucide-react"

import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import { asRecord, extractPlanMarkdown } from "@/lib/plan-parse"
import { MessageResponse } from "@/components/ai-elements/message"
import { cn } from "@/lib/utils"

/**
 * Dedicated rendering for plan-*mode* transition tools — Claude Code's
 * `EnterPlanMode`/`ExitPlanMode` and Cline's `switch_mode`. These are mode
 * signals, not work tools, so they render directly here instead of folding into
 * the misleading "思考 N 次" tool-group (see `isPlanModeToolName` in
 * `plan-parse.ts` and the run-break in `groupConsecutiveToolCalls`).
 *
 * Content-driven, not name-driven:
 *   - input carries freeform plan markdown (ExitPlanMode, or a switch_mode that
 *     proposes a plan) → render the plan directly as a card.
 *   - otherwise → a compact, non-collapsible mode marker. `enterplanmode` and
 *     `exitplanmode` get plan-specific labels; a non-plan `switch_mode` gets a
 *     neutral "switched mode" marker (never mislabeled as a plan).
 */

function parseInput(input: string | null): Record<string, unknown> | null {
  if (!input) return null
  try {
    return asRecord(JSON.parse(input))
  } catch {
    return null
  }
}

function PlanMarkdownCard({
  markdown,
  label,
}: {
  markdown: string
  label: string
}) {
  return (
    <div className="w-full overflow-hidden rounded-md border border-border/60">
      <div className="flex items-center gap-1.5 border-b border-border/60 bg-muted/40 px-3 py-2 text-xs font-medium text-muted-foreground">
        <ListTodoIcon className="size-3.5 shrink-0" />
        {label}
      </div>
      <div className="prose prose-sm max-w-none px-3.5 py-3 text-sm dark:prose-invert [&_ol]:list-inside [&_ul]:list-inside">
        <MessageResponse>{markdown}</MessageResponse>
      </div>
    </div>
  )
}

function PlanModeMarker({ label }: { label: string }) {
  return (
    <div className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-3 py-1.5 text-xs font-medium text-muted-foreground">
      <ListTodoIcon className="size-3.5 shrink-0 opacity-70" />
      {label}
    </div>
  )
}

export const PlanModeCard = memo(function PlanModeCard({
  toolName,
  input,
  errorText,
  state,
}: {
  /** Normalized (tool-call-normalization) form: enterplanmode|exitplanmode|switch_mode. */
  toolName: string
  input: string | null
  errorText: string | null
  state: ToolCallState
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const parsed = parseInput(input)
  const planMarkdown = parsed ? extractPlanMarkdown(parsed) : null
  const isError = state === "output-error" || !!errorText?.trim()

  const markerLabel = (() => {
    if (toolName === "exitplanmode") return t("planMode.submitted")
    if (toolName === "switch_mode") {
      const mode =
        typeof parsed?.mode === "string"
          ? parsed.mode
          : typeof parsed?.mode_slug === "string"
            ? parsed.mode_slug
            : null
      return mode
        ? `${t("planMode.switched")} · ${mode}`
        : t("planMode.switched")
    }
    return t("planMode.entered")
  })()

  return (
    <div className={cn(planMarkdown ? "w-full space-y-2" : "space-y-2")}>
      {planMarkdown ? (
        <PlanMarkdownCard
          markdown={planMarkdown}
          label={t("planMode.planLabel")}
        />
      ) : (
        <PlanModeMarker label={markerLabel} />
      )}
      {isError && errorText && (
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words rounded-md bg-destructive/10 p-3 text-xs text-destructive">
          {errorText}
        </pre>
      )}
    </div>
  )
})
