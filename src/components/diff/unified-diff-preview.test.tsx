import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import { UnifiedDiffPreview } from "./unified-diff-preview"
import enMessages from "@/i18n/messages/en.json"

// The component reads the active folder only to strip a path prefix from the
// file header; a null folder is enough for these tests.
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: null }),
}))

function Wrapper({ children }: { children: React.ReactNode }) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {children}
    </NextIntlClientProvider>
  )
}

// `wrapper` so `rerender` re-applies the intl provider automatically.
function renderWithIntl(ui: React.ReactElement) {
  return render(ui, { wrapper: Wrapper })
}

/** A single-file "add" diff with `lineCount` added rows. */
function newFileDiff(lineCount: number): string {
  const header = `diff --git a/big.txt b/big.txt\n--- /dev/null\n+++ b/big.txt\n@@ -0,0 +1,${lineCount} @@\n`
  const body = Array.from(
    { length: lineCount },
    (_, i) => `+line ${i + 1}`
  ).join("\n")
  return `${header}${body}\n`
}

describe("UnifiedDiffPreview", () => {
  it("renders every row for a small diff and shows no reveal control", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(3)} />)

    expect(screen.getByText("line 1")).toBeInTheDocument()
    expect(screen.getByText("line 3")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more lines/ })
    ).not.toBeInTheDocument()
  })

  it("caps a large diff at 500 rows and offers to reveal the rest", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(600)} />)

    // The first 500 rows render; rows past the cap do not.
    expect(screen.getByText("line 500")).toBeInTheDocument()
    expect(screen.queryByText("line 501")).not.toBeInTheDocument()
    expect(screen.queryByText("line 600")).not.toBeInTheDocument()

    // The reveal control reports exactly the hidden count (600 - 500).
    expect(
      screen.getByRole("button", { name: "Show 100 more lines" })
    ).toBeInTheDocument()
  })

  it("reveals all rows and drops the control once expanded", () => {
    renderWithIntl(<UnifiedDiffPreview diffText={newFileDiff(600)} />)

    fireEvent.click(screen.getByRole("button", { name: "Show 100 more lines" }))

    expect(screen.getByText("line 600")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more lines/ })
    ).not.toBeInTheDocument()
  })

  it("re-caps when the same preview receives a different large diff", () => {
    const { rerender } = renderWithIntl(
      <UnifiedDiffPreview diffText={newFileDiff(600)} />
    )
    // Fully expand the first diff.
    fireEvent.click(screen.getByRole("button", { name: "Show 100 more lines" }))
    expect(screen.getByText("line 600")).toBeInTheDocument()

    // A different, larger diff arrives at the same file position. Sections are
    // keyed positionally, so the instance is reused — the reveal must reset so
    // the new diff is capped again rather than rendered in full.
    rerender(<UnifiedDiffPreview diffText={newFileDiff(700)} />)

    expect(
      screen.getByRole("button", { name: "Show 200 more lines" })
    ).toBeInTheDocument()
    expect(screen.getByText("line 500")).toBeInTheDocument()
    expect(screen.queryByText("line 700")).not.toBeInTheDocument()
  })
})
