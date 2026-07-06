"use client"

import { useState, useMemo } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { FolderOpen, Loader2 } from "lucide-react"
import { cloneRepository } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { isDesktop, openFileDialog } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useGitCredential } from "@/contexts/git-credential-context"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Label } from "@/components/ui/label"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"

interface CloneDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function CloneDialog({ open, onOpenChange }: CloneDialogProps) {
  const t = useTranslations("Folder.cloneDialog")
  const tToasts = useTranslations("Folder.toasts")
  const openFolder = useAppWorkspaceStore((s) => s.openFolder)
  const { withCredentialRetry } = useGitCredential()
  const [url, setUrl] = useState("")
  const [targetDir, setTargetDir] = useState("")
  const [cloning, setCloning] = useState(false)
  const [browserOpen, setBrowserOpen] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const repoName = useMemo(
    () =>
      url
        .replace(/\.git$/, "")
        .split("/")
        .pop() ?? "repo",
    [url]
  )

  const handleBrowse = async () => {
    // Clone happens on the remote host when bound to a remote workspace —
    // the target dir must therefore live on that host, not on the local
    // desktop. Route to the in-app browser unless we're truly local.
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      const selected = await openFileDialog({
        directory: true,
        multiple: false,
      })
      if (selected) {
        setTargetDir(Array.isArray(selected) ? selected[0] : selected)
      }
    } else {
      setBrowserOpen(true)
    }
  }

  const resetForm = () => {
    setUrl("")
    setTargetDir("")
    setError(null)
  }

  const handleClone = async () => {
    if (!url || !targetDir) return
    const fullPath = `${targetDir}/${repoName}`
    setCloning(true)
    setError(null)
    try {
      await withCredentialRetry(
        (creds) => cloneRepository(url, fullPath, creds),
        { remoteUrl: url }
      )
      await openFolder(fullPath)
      onOpenChange(false)
      resetForm()
    } catch (err) {
      const msg = toErrorMessage(err)
      setError(msg)
      toast.error(tToasts("cloneFailed"), { description: msg })
    } finally {
      setCloning(false)
    }
  }

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={(v) => {
          onOpenChange(v)
          if (!v) resetForm()
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("title")}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-2">
              <Label htmlFor="clone-repo-url">{t("repositoryUrl")}</Label>
              <Input
                id="clone-repo-url"
                placeholder={t("repositoryUrlPlaceholder")}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                disabled={cloning}
                autoFocus
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="clone-target-dir">{t("directory")}</Label>
              <div className="flex gap-2">
                <Input
                  id="clone-target-dir"
                  placeholder={t("directoryPlaceholder")}
                  value={targetDir}
                  onChange={(e) => setTargetDir(e.target.value)}
                  disabled={cloning}
                  className="flex-1"
                />
                <Button
                  variant="outline"
                  size="icon"
                  onClick={handleBrowse}
                  disabled={cloning}
                  title={t("browseDirectory")}
                  aria-label={t("browseDirectory")}
                  type="button"
                >
                  <FolderOpen className="h-4 w-4" />
                </Button>
              </div>
              {targetDir && url && (
                <p className="text-xs text-muted-foreground">
                  {t("clonePath", { path: `${targetDir}/${repoName}` })}
                </p>
              )}
            </div>
            {error && <p className="text-sm text-destructive">{error}</p>}
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => onOpenChange(false)}
              disabled={cloning}
              type="button"
            >
              {t("cancel")}
            </Button>
            <Button
              onClick={handleClone}
              disabled={!url || !targetDir || cloning}
              type="button"
            >
              {cloning && <Loader2 className="h-4 w-4 mr-2 animate-spin" />}
              {t("clone")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={(path) => setTargetDir(path)}
      />
    </>
  )
}
