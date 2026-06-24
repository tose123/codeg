import { type ReactElement } from "react"
import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { AgentToolCallPart } from "./agent-tool-call"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import enMessages from "@/i18n/messages/en.json"

type ToolCallPart = Extract<AdaptedContentPart, { type: "tool-call" }>

function renderCard(part: ToolCallPart) {
  const ui: ReactElement = (
    <AgentToolCallPart part={part} renderToolCall={() => null} />
  )
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function basePart(
  input: string | null,
  state: ToolCallPart["state"]
): ToolCallPart {
  return {
    type: "tool-call",
    toolCallId: "call-agent",
    toolName: "agent",
    input,
    state,
  }
}

describe("AgentToolCallPart title", () => {
  it("renders the subagent_type prefix in front of the description", () => {
    renderCard(
      basePart(
        JSON.stringify({
          subagent_type: "Explore",
          description: "map the repo",
        }),
        "input-available"
      )
    )
    expect(screen.getByText("Explore: map the repo")).toBeInTheDocument()
    expect(screen.queryByText(/Sub-agent starting/)).not.toBeInTheDocument()
  })

  it("shows the description alone when subagent_type hasn't streamed in yet", () => {
    // Partial / out-of-order streamed input: the description is present but the
    // sub-agent type isn't. The placeholder must NOT be prepended to it.
    renderCard(
      basePart(
        '{"description":"map the repo"', // truncated, no subagent_type yet
        "input-streaming"
      )
    )
    expect(screen.getByText("map the repo")).toBeInTheDocument()
    expect(screen.queryByText(/Sub-agent starting/)).not.toBeInTheDocument()
  })

  it("falls back to the placeholder only when nothing has arrived", () => {
    renderCard(basePart(null, "input-available"))
    expect(screen.getByText("Sub-agent starting…")).toBeInTheDocument()
  })

  it("reads Codex's agent_type field as the prefix", () => {
    // Codex's live spawn_agent payload labels the agent with `agent_type`.
    renderCard(
      basePart(
        JSON.stringify({ agent_type: "codex", description: "do the thing" }),
        "input-available"
      )
    )
    expect(screen.getByText("codex: do the thing")).toBeInTheDocument()
  })

  it("ignores non-string subagent_type / description (no React-child crash)", () => {
    // Some hosts (e.g. CodeBuddy) can hand us a tool input where these fields
    // are objects, not strings. Rendering them directly would throw "Objects
    // are not valid as a React child"; they must be treated as absent.
    expect(() =>
      renderCard(
        basePart(
          JSON.stringify({ subagent_type: {}, description: {} }),
          "input-available"
        )
      )
    ).not.toThrow()
    expect(screen.getByText("Sub-agent starting…")).toBeInTheDocument()
  })

  it("keeps a string description when subagent_type is a non-string object", () => {
    renderCard(
      basePart(
        JSON.stringify({
          subagent_type: { nested: true },
          description: "build",
        }),
        "input-available"
      )
    )
    expect(screen.getByText("build")).toBeInTheDocument()
  })
})
