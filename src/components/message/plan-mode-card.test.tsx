import { render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

// Exercise the real Streamdown pipeline for the plan-markdown branch; only the
// link-safety hook is stubbed (no bearing on plan rendering), mirroring
// message-softbreaks.test.tsx.
vi.mock("@/components/ai-elements/link-safety", () => ({
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

import { PlanModeCard } from "./plan-mode-card"
import enMessages from "@/i18n/messages/en.json"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

function renderCard(props: {
  toolName: string
  input: string | null
  errorText?: string | null
  state?: ToolCallState
}) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <PlanModeCard
        toolName={props.toolName}
        input={props.input}
        errorText={props.errorText ?? null}
        state={props.state ?? "output-available"}
      />
    </NextIntlClientProvider>
  )
}

describe("PlanModeCard", () => {
  it("renders a compact marker for EnterPlanMode (no plan body)", () => {
    const { container } = renderCard({ toolName: "enterplanmode", input: "{}" })
    expect(screen.getByText("Entered plan mode")).toBeInTheDocument()
    // No collapsible Tool shell, no plan card header.
    expect(screen.queryByText("Plan")).toBeNull()
    expect(container.querySelector("pre")).toBeNull()
  })

  it("renders ExitPlanMode's plan markdown directly under a Plan label", async () => {
    const { container } = renderCard({
      toolName: "exitplanmode",
      input: JSON.stringify({ plan: "# Heading\n- item one" }),
    })
    expect(screen.getByText("Plan")).toBeInTheDocument()
    await waitFor(() => {
      expect(container.textContent).toContain("Heading")
      expect(container.textContent).toContain("item one")
    })
  })

  it("renders a neutral marker (with target mode) for a non-plan switch_mode", () => {
    renderCard({
      toolName: "switch_mode",
      input: JSON.stringify({ mode: "act" }),
    })
    // Content-driven: no plan markdown → mode marker, NOT a "Plan" card.
    expect(screen.getByText("Switched mode · act")).toBeInTheDocument()
    expect(screen.queryByText("Plan")).toBeNull()
  })

  it("renders the plan when switch_mode carries plan markdown (content-driven)", async () => {
    const { container } = renderCard({
      toolName: "switch_mode",
      input: JSON.stringify({ mode: "plan", plan: "## Steps\n- do x" }),
    })
    expect(screen.getByText("Plan")).toBeInTheDocument()
    await waitFor(() => {
      expect(container.textContent).toContain("Steps")
      expect(container.textContent).toContain("do x")
    })
    expect(screen.queryByText("Switched mode · plan")).toBeNull()
  })

  it("surfaces the error text on a failed plan-mode call", () => {
    renderCard({
      toolName: "exitplanmode",
      input: "{}",
      errorText: "boom",
      state: "output-error",
    })
    expect(screen.getByText("boom")).toBeInTheDocument()
  })
})
