import { act, render, waitFor, cleanup } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"

// The workspace half lives in the real zustand store: tests seed it via
// setState in beforeEach and flip hydration with act(setState). The tab half
// is still a mutable hook mock — the mock reads this module-level var, so
// reassigning + rerendering simulates the provider state changing.
let tabs: { tabsHydrated: boolean; openTab: ReturnType<typeof vi.fn> }
let addFolderToWorkspaceById: ReturnType<typeof vi.fn>
let capturedHandler: ((p: unknown) => void) | null = null

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (selector: (s: typeof tabs) => unknown) => selector(tabs),
  useTabActions: () => tabs,
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({
    subscribe: async (_event: string, cb: (p: unknown) => void) => {
      capturedHandler = cb
      return () => {}
    },
  }),
}))

import { PetFocusBridge } from "./deep-link-bootstrap"

describe("PetFocusBridge", () => {
  beforeEach(() => {
    capturedHandler = null
    addFolderToWorkspaceById = vi.fn()
    resetAppWorkspaceStore()
    useAppWorkspaceStore.setState({
      foldersHydrated: false,
      folders: [{ id: 7 }] as never,
      addFolderToWorkspaceById,
    })
    tabs = { tabsHydrated: false, openTab: vi.fn() }
  })
  afterEach(() => cleanup())

  it("queues a request that arrives before hydration and replays it", async () => {
    const { rerender } = render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    // Arrives before folders/tabs (and the independently-loading conversations
    // snapshot) are ready — must not be dropped.
    capturedHandler!({ folderId: 7, conversationId: 42, agent: "claude_code" })
    expect(tabs.openTab).not.toHaveBeenCalled()

    // Hydration completes → queued request replays.
    tabs = { ...tabs, tabsHydrated: true }
    rerender(<PetFocusBridge />)
    act(() => {
      useAppWorkspaceStore.setState({ foldersHydrated: true })
    })

    await waitFor(() =>
      expect(tabs.openTab).toHaveBeenCalledWith(7, 42, "claude_code", true)
    )
  })

  it("opens immediately when already hydrated, without re-adding an open folder", async () => {
    useAppWorkspaceStore.setState({ foldersHydrated: true })
    tabs = { ...tabs, tabsHydrated: true }
    render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    capturedHandler!({ folderId: 7, conversationId: 9, agent: "codex" })
    await waitFor(() =>
      expect(tabs.openTab).toHaveBeenCalledWith(7, 9, "codex", true)
    )
    expect(addFolderToWorkspaceById).not.toHaveBeenCalled()
  })

  it("ignores malformed payloads", async () => {
    useAppWorkspaceStore.setState({ foldersHydrated: true })
    tabs = { ...tabs, tabsHydrated: true }
    render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    capturedHandler!({ folderId: "x", conversationId: 1, agent: "codex" })
    capturedHandler!({ folderId: 7, conversationId: 1 }) // missing agent
    await Promise.resolve()
    expect(tabs.openTab).not.toHaveBeenCalled()
  })
})
