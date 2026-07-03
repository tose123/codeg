import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({ useTranslations: () => (k: string) => k }))
vi.mock("@/lib/api", () => ({
  startOfficeWatch: vi.fn(),
  stopOfficeWatch: vi.fn(),
  openSettingsWindow: vi.fn(),
}))
vi.mock("@/lib/transport", () => ({
  isDesktop: vi.fn(),
  isRemoteDesktopMode: vi.fn(),
  getServerBaseUrl: vi.fn(),
}))

import {
  openSettingsWindow,
  startOfficeWatch,
  stopOfficeWatch,
} from "@/lib/api"
import {
  isDesktop,
  isRemoteDesktopMode,
  getServerBaseUrl,
} from "@/lib/transport"
import { OfficePreview } from "./office-preview"

const mockStart = vi.mocked(startOfficeWatch)
const mockStop = vi.mocked(stopOfficeWatch)
const mockOpenSettings = vi.mocked(openSettingsWindow)
const mockIsDesktop = vi.mocked(isDesktop)
const mockIsRemote = vi.mocked(isRemoteDesktopMode)
const mockBaseUrl = vi.mocked(getServerBaseUrl)

beforeEach(() => {
  vi.clearAllMocks()
  mockStop.mockResolvedValue(undefined)
  mockOpenSettings.mockResolvedValue(undefined)
  mockIsRemote.mockReturnValue(false)
  mockBaseUrl.mockReturnValue("https://srv.example")
})
afterEach(() => cleanup())

describe("OfficePreview", () => {
  it("web mode: proxies through the server with the cap, opaque-origin sandbox", async () => {
    mockIsDesktop.mockReturnValue(false)
    mockStart.mockResolvedValue({ port: 26315, cap: "capval" })

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    await waitFor(() =>
      expect(mockStart).toHaveBeenCalledWith("/root/reports", "a.docx")
    )
    const iframe = await screen.findByTitle("officePreviewTitle")
    expect(iframe.getAttribute("src")).toBe(
      "https://srv.example/api/office-watch-proxy/26315/?cap=capval"
    )
    // No allow-same-origin in web mode → the page can't read app storage.
    expect(iframe.getAttribute("sandbox")).toBe(
      "allow-scripts allow-popups allow-forms"
    )
  })

  it("local desktop: loads loopback directly with same-origin sandbox", async () => {
    mockIsDesktop.mockReturnValue(true)
    mockIsRemote.mockReturnValue(false)
    mockStart.mockResolvedValue({ port: 30000, cap: "ignored" })

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    const iframe = await screen.findByTitle("officePreviewTitle")
    expect(iframe.getAttribute("src")).toBe("http://127.0.0.1:30000/")
    expect(iframe.getAttribute("sandbox")).toContain("allow-same-origin")
  })

  it("remote-desktop: shows a hint and does NOT start a watch", async () => {
    // A Tauri window bound to a remote server can't load the preview iframe
    // (mixed-content); we must not spawn a remote watch nobody can see.
    mockIsDesktop.mockReturnValue(true)
    mockIsRemote.mockReturnValue(true)

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    expect(
      await screen.findByText("officeRemoteDesktopUnsupported")
    ).toBeTruthy()
    expect(mockStart).not.toHaveBeenCalled()
  })

  it("stops the watch on unmount", async () => {
    mockIsDesktop.mockReturnValue(false)
    mockStart.mockResolvedValue({ port: 1, cap: "c" })

    const { unmount } = render(
      <OfficePreview rootPath="/root/reports" relPath="a.docx" />
    )
    await screen.findByTitle("officePreviewTitle")
    unmount()

    expect(mockStop).toHaveBeenCalledWith("/root/reports", "a.docx")
  })

  it("desktop NOT_INSTALLED: offers Open Settings", async () => {
    mockIsDesktop.mockReturnValue(true)
    mockIsRemote.mockReturnValue(false)
    mockStart.mockRejectedValue({
      code: "dependency_missing",
      message: "officecli is not installed",
      i18n_params: { watchCode: "NOT_INSTALLED" },
    })

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    const btn = await screen.findByText("officeOpenSettings")
    fireEvent.click(btn)
    expect(mockOpenSettings).toHaveBeenCalled()
  })

  it("web NOT_INSTALLED: shows the server-side install hint instead", async () => {
    mockIsDesktop.mockReturnValue(false)
    mockStart.mockRejectedValue({
      code: "dependency_missing",
      message: "officecli is not installed",
      i18n_params: { watchCode: "NOT_INSTALLED" },
    })

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    expect(await screen.findByText("officeServerInstallHint")).toBeTruthy()
    expect(screen.queryByText("officeOpenSettings")).toBeNull()
  })

  it("offers retry for a START_FAILED error and re-starts the watch", async () => {
    mockIsDesktop.mockReturnValue(false)
    mockStart.mockRejectedValueOnce({
      code: "task_execution_failed",
      message: "boom",
      i18n_params: { watchCode: "START_FAILED" },
    })
    mockStart.mockResolvedValueOnce({ port: 5, cap: "c" })

    render(<OfficePreview rootPath="/root/reports" relPath="a.docx" />)

    const retry = await screen.findByText("officeWatchRetry")
    expect(mockStart).toHaveBeenCalledTimes(1)
    fireEvent.click(retry)
    await waitFor(() => expect(mockStart).toHaveBeenCalledTimes(2))
  })
})
