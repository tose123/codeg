import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import { PermissionDialog } from "./permission-dialog"
import enMessages from "@/i18n/messages/en.json"
import type { PendingPermission } from "@/contexts/acp-connections-context"

// MessageResponse (Streamdown) pulls in async Shiki highlighting + the
// link-safety hook — too heavy for this unit test, and its Markdown correctness
// is covered elsewhere. Stub it to a sentinel that echoes its source so we can
// assert the agent's content text is routed THROUGH the Markdown renderer
// rather than dumped as raw JSON.
vi.mock("@/components/ai-elements/message", () => ({
  MessageResponse: ({ children }: { children: string }) => (
    <div data-testid="markdown-response">{children}</div>
  ),
}))

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const baseOptions = [
  { option_id: "allow", name: "Allow once", kind: "allow_once" },
  { option_id: "reject", name: "Reject", kind: "reject_once" },
]

describe("PermissionDialog", () => {
  it("returns nothing when permission is null", () => {
    const { container } = renderWithIntl(
      <PermissionDialog permission={null} onRespond={() => {}} />
    )
    expect(container.firstChild).toBeNull()
  })

  it("renders the parsed title and the english subtitle copy", () => {
    const permission: PendingPermission = {
      request_id: "req-1",
      tool_call: { title: "Run unit tests", kind: "shell" },
      options: baseOptions,
    }
    renderWithIntl(
      <PermissionDialog permission={permission} onRespond={() => {}} />
    )
    expect(screen.getByText("Run unit tests")).toBeInTheDocument()
    expect(
      screen.getByText("Agent requests permission to continue this turn.")
    ).toBeInTheDocument()
  })

  it("renders every option as a button", () => {
    const permission: PendingPermission = {
      request_id: "req-2",
      tool_call: null,
      options: baseOptions,
    }
    renderWithIntl(
      <PermissionDialog permission={permission} onRespond={() => {}} />
    )
    expect(
      screen.getByRole("button", { name: "Allow once" })
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Reject" })).toBeInTheDocument()
  })

  it("invokes onRespond with the request_id + chosen option_id when clicked", () => {
    const onRespond = vi.fn()
    const permission: PendingPermission = {
      request_id: "req-abc",
      tool_call: null,
      options: baseOptions,
    }
    renderWithIntl(
      <PermissionDialog permission={permission} onRespond={onRespond} />
    )
    fireEvent.click(screen.getByRole("button", { name: "Allow once" }))
    expect(onRespond).toHaveBeenCalledWith("req-abc", "allow")
  })

  it("falls back to a JSON preview when the tool_call has no structured fields", () => {
    // Tool calls with no command / file changes / plan / web / etc. should
    // hit the `jsonPreview` branch so the user still sees raw input.
    const permission: PendingPermission = {
      request_id: "req-3",
      tool_call: { kind: "unknown_tool", payload: { hello: "world" } },
      options: baseOptions,
    }
    const { container } = renderWithIntl(
      <PermissionDialog permission={permission} onRespond={() => {}} />
    )
    const pre = container.querySelector("pre")
    expect(pre).not.toBeNull()
    expect(pre?.textContent).toContain("hello")
    expect(pre?.textContent).toContain("world")
  })

  it("renders the agent description from ACP content text instead of raw JSON", () => {
    // Kimi Code carries the request description in the ACP `content` array
    // (`{ type: "content", content: { type: "text", text } }`) and populates
    // nothing in rawInput. The dialog should surface that text via the Markdown
    // renderer rather than dumping the whole tool_call as raw JSON.
    const permission: PendingPermission = {
      request_id: "req-kimi",
      tool_call: {
        content: [
          {
            type: "content",
            content: {
              type: "text",
              text: "Requesting approval to Running: mkdir -p app/todo-test",
            },
          },
        ],
        title: "Bash",
        toolCallId: "0:Bash_8",
        kind: "execute",
      },
      options: baseOptions,
    }
    const { container } = renderWithIntl(
      <PermissionDialog permission={permission} onRespond={() => {}} />
    )
    const markdown = screen.getByTestId("markdown-response")
    expect(markdown).toHaveTextContent(
      "Requesting approval to Running: mkdir -p app/todo-test"
    )
    expect(container.querySelector("pre")).toBeNull()
  })
})
