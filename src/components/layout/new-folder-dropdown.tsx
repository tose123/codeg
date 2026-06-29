"use client"

import { useState } from "react"
import { FolderGit2, FolderOpenDot, FolderPlus, Rocket } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Button } from "@/components/ui/button"
import { openProjectBootWindow } from "@/lib/api"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { CloneDialog } from "@/components/layout/clone-dialog"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"

export function NewFolderDropdown() {
  const t = useTranslations("Folder.folderNameDropdown")
  const { openFolder } = useAppWorkspace()
  const [cloneOpen, setCloneOpen] = useState(false)
  const [browserOpen, setBrowserOpen] = useState(false)

  async function handleOpenFolder() {
    // Only use the native Tauri directory dialog when running on the local
    // desktop. In a remote workspace window we're still inside Tauri, but the
    // folder we want lives on the remote host — the native dialog would
    // browse the *local* filesystem and produce a path the remote server
    // can't open. Fall through to the in-app server-side browser instead.
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      const selected = await openFileDialog({
        directory: true,
        multiple: false,
      })
      if (selected) {
        await openFolder(Array.isArray(selected) ? selected[0] : selected)
      }
    } else {
      setBrowserOpen(true)
    }
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6 hover:text-foreground/80"
            title={t("openFolder")}
          >
            <FolderPlus className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="min-w-56" align="start">
          <DropdownMenuItem onSelect={handleOpenFolder}>
            <FolderOpenDot className="h-3.5 w-3.5 shrink-0" />
            {t("openFolder")}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setCloneOpen(true)}>
            <FolderGit2 className="h-3.5 w-3.5 shrink-0" />
            {t("cloneRepository")}
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => openProjectBootWindow()}>
            <Rocket className="h-3.5 w-3.5 shrink-0" />
            {t("projectBoot")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <CloneDialog open={cloneOpen} onOpenChange={setCloneOpen} />
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        allowCreateDirectory
        onSelect={(path) => {
          openFolder(path).catch((err) => {
            console.error("[NewFolderDropdown] failed to open folder:", err)
          })
        }}
      />
    </>
  )
}
