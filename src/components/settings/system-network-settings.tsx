"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import {
  ArrowUpCircle,
  CheckCircle2,
  Languages,
  Loader2,
  RefreshCw,
  Wifi,
} from "lucide-react"
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Update = any
import { useLocale, useTranslations } from "next-intl"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { toast } from "sonner"
import { useAppI18n } from "@/components/i18n-provider"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  getSystemProxySettings,
  updateSystemLanguageSettings,
  updateSystemProxySettings,
} from "@/lib/api"
import { isDesktop, openUrl } from "@/lib/platform"
import type { AppLocale } from "@/lib/types"
import {
  checkAppUpdate,
  closeAppUpdate,
  getCurrentAppVersion,
  installAppUpdate,
  normalizeAppUpdateError,
  performServerUpdate,
  relaunchApp,
  restartServer,
  subscribeServerUpdateProgress,
  waitForServerHealthy,
} from "@/lib/updater"
import type { DownloadEvent, ServerUpdateProgress } from "@/lib/updater"
import { APP_LOCALES } from "@/lib/i18n"
import { toErrorMessage } from "@/lib/app-error"

function GithubMarkIcon({ className }: { className?: string }) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      fill="currentColor"
      fillRule="evenodd"
      viewBox="0 0 24 24"
      xmlns="http://www.w3.org/2000/svg"
    >
      <path d="M12 0c6.63 0 12 5.276 12 11.79-.001 5.067-3.29 9.567-8.175 11.187-.6.118-.825-.25-.825-.56 0-.398.015-1.665.015-3.242 0-1.105-.375-1.813-.81-2.181 2.67-.295 5.475-1.297 5.475-5.822 0-1.297-.465-2.344-1.23-3.169.12-.295.54-1.503-.12-3.125 0 0-1.005-.324-3.3 1.209a11.32 11.32 0 00-3-.398c-1.02 0-2.04.133-3 .398-2.295-1.518-3.3-1.209-3.3-1.209-.66 1.622-.24 2.83-.12 3.125-.765.825-1.23 1.887-1.23 3.169 0 4.51 2.79 5.527 5.46 5.822-.345.294-.66.81-.765 1.577-.69.31-2.415.81-3.495-.973-.225-.354-.9-1.223-1.845-1.209-1.005.015-.405.56.015.781.51.28 1.095 1.327 1.23 1.666.24.663 1.02 1.93 4.035 1.385 0 .988.015 1.916.015 2.196 0 .31-.225.664-.825.56C3.303 21.374-.003 16.867 0 11.791 0 5.276 5.37 0 12 0z" />
    </svg>
  )
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

const PROXY_EXAMPLE = "http://127.0.0.1:7890"
const APP_LANGUAGE_VALUES = APP_LOCALES

type LanguageSelectValue = "system" | AppLocale

function isAppLocale(value: string): value is AppLocale {
  return APP_LANGUAGE_VALUES.includes(value as AppLocale)
}

type UpdateAction = "check" | "install"

export function SystemNetworkSettings() {
  const t = useTranslations("SystemSettings")
  const tLanguage = useTranslations("Language")
  const locale = useLocale()
  const { languageSettings, languageSettingsLoaded, setLanguageSettings } =
    useAppI18n()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [savingLanguage, setSavingLanguage] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [proxyUrl, setProxyUrl] = useState("")
  const [proxyUrlError, setProxyUrlError] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [currentVersion, setCurrentVersion] = useState<string>("")
  const [availableUpdate, setAvailableUpdate] = useState<Update | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [installingUpdate, setInstallingUpdate] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [lastCheckedAt, setLastCheckedAt] = useState<Date | null>(null)
  const [downloadProgress, setDownloadProgress] = useState<{
    downloaded: number
    total: number | null
    phase: "downloading" | "installing"
  } | null>(null)
  // Server/Docker self-update capability reported by `check_app_update`
  // (absent in desktop mode). Drives whether the upgrade button performs a
  // real in-place update or just links to the release page.
  const [serverSelfUpdate, setServerSelfUpdate] = useState(false)
  const [serverRuntime, setServerRuntime] = useState<string | undefined>(
    undefined
  )
  const [restartDelayMs, setRestartDelayMs] = useState(2000)
  // Non-null while the server is restarting after an upgrade: the seconds
  // remaining on the countdown before we start polling /health.
  const [restartCountdown, setRestartCountdown] = useState<number | null>(null)

  const [appLanguage, setAppLanguage] = useState<LanguageSelectValue>(
    languageSettings.mode === "system" ? "system" : languageSettings.language
  )

  useEffect(() => {
    setAppLanguage(
      languageSettings.mode === "system" ? "system" : languageSettings.language
    )
  }, [languageSettings])

  const languageLabels = useMemo(
    () => ({
      en: tLanguage("english"),
      zh_cn: tLanguage("simplifiedChinese"),
      zh_tw: tLanguage("traditionalChinese"),
      ja: tLanguage("japanese"),
      ko: tLanguage("korean"),
      es: tLanguage("spanish"),
      de: tLanguage("german"),
      fr: tLanguage("french"),
      pt: tLanguage("portuguese"),
      ar: tLanguage("arabic"),
    }),
    [tLanguage]
  )

  const formattedLastCheckedAt = useMemo(() => {
    if (!lastCheckedAt) return null
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(lastCheckedAt)
  }, [lastCheckedAt, locale])

  const formattedUpdateDate = useMemo(() => {
    if (!availableUpdate?.date) return null

    const parsed = new Date(availableUpdate.date)
    if (Number.isNaN(parsed.getTime())) return availableUpdate.date

    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
    }).format(parsed)
  }, [availableUpdate?.date, locale])

  const updateNotes = useMemo(
    () => availableUpdate?.body?.trim() ?? "",
    [availableUpdate?.body]
  )

  const updateStatusMessage = useMemo(() => {
    if (checkingUpdate) return t("checking")
    if (installingUpdate) return t("updating")
    if (availableUpdate) return null
    if (lastCheckedAt) return t("alreadyLatest")
    return null
  }, [availableUpdate, checkingUpdate, installingUpdate, lastCheckedAt, t])

  const loadSettings = useCallback(async () => {
    setLoading(true)
    setLoadError(null)

    try {
      const [proxySettings, version] = await Promise.all([
        getSystemProxySettings(),
        getCurrentAppVersion(),
      ])

      setEnabled(proxySettings.enabled)
      setProxyUrl(proxySettings.proxy_url ?? "")
      setCurrentVersion(version)
    } catch (err) {
      const message = toErrorMessage(err)
      setLoadError(message)
      console.error("[Settings] load system settings failed:", err)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    loadSettings().catch((err) => {
      console.error("[Settings] load system settings failed:", err)
    })
    checkForUpdates().catch((err) => {
      console.error("[Settings] auto check update failed:", err)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    return () => {
      if (!availableUpdate) return
      closeAppUpdate(availableUpdate).catch((err) => {
        console.error("[Settings] release updater resource failed:", err)
      })
    }
  }, [availableUpdate])

  const saveProxySettings = useCallback(
    async (nextEnabled: boolean, nextProxyUrl: string) => {
      if (nextEnabled && !nextProxyUrl.trim()) return

      setSaving(true)
      try {
        const next = await updateSystemProxySettings({
          enabled: nextEnabled,
          proxy_url: nextProxyUrl.trim() || null,
        })
        setEnabled(next.enabled)
        setProxyUrl(next.proxy_url ?? "")
      } catch (err) {
        const message = toErrorMessage(err)
        toast.error(t("saveFailed", { message }))
      } finally {
        setSaving(false)
      }
    },
    [t]
  )

  const saveLanguage = useCallback(
    async (lang: LanguageSelectValue) => {
      setSavingLanguage(true)

      try {
        const next = await updateSystemLanguageSettings({
          mode: lang === "system" ? "system" : "manual",
          language: lang === "system" ? languageSettings.language : lang,
        })

        setLanguageSettings(next)
      } catch (err) {
        const message = toErrorMessage(err)
        toast.error(t("languageSaveFailed", { message }))
      } finally {
        setSavingLanguage(false)
      }
    },
    [languageSettings.language, setLanguageSettings, t]
  )

  const formatUpdateError = useCallback(
    (error: unknown, action: UpdateAction): string => {
      const { kind, rawMessage } = normalizeAppUpdateError(error)

      switch (kind) {
        case "source_unreachable":
          return t("updateErrors.sourceUnavailable")
        case "network":
          return t("updateErrors.network")
        case "download_failed":
          return t("updateErrors.downloadFailed")
        case "install_failed":
          return t("updateErrors.installFailed")
        case "unknown":
        default:
          if (action === "install") {
            return t("updateErrors.installFailed")
          }
          console.error("[Settings] updater unknown error:", rawMessage)
          return t("updateErrors.unknown")
      }
    },
    [t]
  )

  const checkForUpdates = useCallback(async () => {
    setCheckingUpdate(true)
    setUpdateError(null)

    try {
      const previousUpdate = availableUpdate
      const result = await checkAppUpdate()
      setCurrentVersion(result.currentVersion)
      setLastCheckedAt(new Date())

      // Server-mode capability (undefined in desktop, where Tauri's updater
      // handles installs and these fields aren't present).
      setServerSelfUpdate(result.selfUpdateSupported ?? false)
      setServerRuntime(result.runtime)
      if (typeof result.restartDelayMs === "number") {
        setRestartDelayMs(result.restartDelayMs)
      }

      if (result.update) {
        setAvailableUpdate(result.update)
      } else {
        setAvailableUpdate(null)
      }

      if (previousUpdate && previousUpdate !== result.update) {
        await closeAppUpdate(previousUpdate)
      }
    } catch (err) {
      const message = formatUpdateError(err, "check")
      setUpdateError(message)
      toast.error(t("checkUpdateFailed", { message }))
      console.error("[Settings] check app update failed:", err)
    } finally {
      setCheckingUpdate(false)
    }
  }, [availableUpdate, formatUpdateError, t])

  const installUpdate = useCallback(async () => {
    if (!availableUpdate) return

    setInstallingUpdate(true)
    setUpdateError(null)
    setDownloadProgress(null)

    let downloaded = 0

    try {
      await installAppUpdate(availableUpdate, (event: DownloadEvent) => {
        switch (event.event) {
          case "Started":
            setDownloadProgress({
              downloaded: 0,
              total: event.data.contentLength ?? null,
              phase: "downloading",
            })
            break
          case "Progress":
            downloaded += event.data.chunkLength
            setDownloadProgress((prev) => ({
              downloaded,
              total: prev?.total ?? null,
              phase: "downloading",
            }))
            break
          case "Finished":
            setDownloadProgress((prev) => ({
              downloaded: prev?.downloaded ?? downloaded,
              total: prev?.total ?? null,
              phase: "installing",
            }))
            break
        }
      })
      toast.success(t("installSuccess"))
      await relaunchApp()
    } catch (err) {
      const message = formatUpdateError(err, "install")
      setUpdateError(message)
      toast.error(t("installFailed", { message }))
      console.error("[Settings] install app update failed:", err)
    } finally {
      setInstallingUpdate(false)
      setDownloadProgress(null)
    }
  }, [availableUpdate, formatUpdateError, t])

  // Server / Docker in-place upgrade: download+verify+swap on the server,
  // ask it to restart, then count down and poll /health before reloading.
  const runServerUpgrade = useCallback(async () => {
    if (!availableUpdate) return

    setInstallingUpdate(true)
    setUpdateError(null)
    setDownloadProgress(null)
    setRestartCountdown(null)

    let unsubscribe: (() => void) | undefined
    try {
      unsubscribe = await subscribeServerUpdateProgress(
        (p: ServerUpdateProgress) => {
          setDownloadProgress({
            downloaded: p.downloaded,
            total: p.total,
            phase: p.phase === "downloading" ? "downloading" : "installing",
          })
        }
      )

      const result = await performServerUpdate()
      setDownloadProgress(null)

      // Trigger the relaunch; the server responds before it exits/re-execs.
      await restartServer()
      const delayMs = result.restartDelayMs || restartDelayMs

      // Phase 1: visible countdown over the configured relaunch delay.
      const totalWaitMs = delayMs + 1000
      const start = Date.now()
      await new Promise<void>((resolve) => {
        const tick = () => {
          const remaining = Math.max(0, totalWaitMs - (Date.now() - start))
          setRestartCountdown(Math.ceil(remaining / 1000))
          if (remaining <= 0) resolve()
          else setTimeout(tick, 250)
        }
        tick()
      })

      // Phase 2: poll until the new process answers (or give up).
      const healthy = await waitForServerHealthy({
        timeoutMs: 90_000,
        intervalMs: 1500,
      })
      if (!healthy) {
        setUpdateError(t("restartTimeout"))
        toast.error(t("restartTimeout"))
        return
      }
      // The server answered — but the supervisor may have auto-rolled-back a
      // version that couldn't boot, in which case the OLD version is now
      // serving and healthy. Confirm the running version actually advanced
      // before claiming success.
      let rolledBack = false
      if (result.version) {
        try {
          const after = await checkAppUpdate()
          rolledBack = after.currentVersion !== result.version
        } catch {
          // Re-check failed; don't block — assume the upgrade applied.
        }
      }
      if (rolledBack) {
        setUpdateError(t("upgradeRolledBack"))
        toast.error(t("upgradeRolledBack"))
      } else {
        toast.success(t("upgradeSuccess"))
        window.location.reload()
      }
    } catch (err) {
      const message = formatUpdateError(err, "install")
      setUpdateError(message)
      toast.error(t("installFailed", { message }))
      console.error("[Settings] server upgrade failed:", err)
    } finally {
      unsubscribe?.()
      setInstallingUpdate(false)
      setDownloadProgress(null)
      setRestartCountdown(null)
    }
  }, [availableUpdate, formatUpdateError, restartDelayMs, t])

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        <section className="space-y-1">
          <div className="flex items-center justify-between">
            <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
            <Button
              variant="ghost"
              className="size-5 rounded-full"
              onClick={() => openUrl("https://github.com/xintaofei/codeg")}
            >
              <GithubMarkIcon className="size-5" />
            </Button>
          </div>
          <p className="text-xs text-muted-foreground">
            {t("sectionDescription")}
          </p>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            {checkingUpdate ? (
              <RefreshCw className="h-4 w-4 text-muted-foreground animate-spin" />
            ) : availableUpdate ? (
              <ArrowUpCircle className="h-4 w-4 text-muted-foreground" />
            ) : lastCheckedAt ? (
              <CheckCircle2 className="h-4 w-4 text-green-500" />
            ) : (
              <RefreshCw className="h-4 w-4 text-muted-foreground" />
            )}
            <h2 className="text-sm font-semibold">{t("versionTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("updateDescription")}
          </p>

          <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-2">
            <div className="flex items-center justify-between gap-3">
              <p className="text-muted-foreground">
                {t("currentVersion")}：
                {currentVersion ? `v${currentVersion}` : "-"}
              </p>
              {checkingUpdate ? (
                <Button
                  key="checking-update"
                  size="sm"
                  disabled
                  aria-busy="true"
                  className="w-[9.5rem] justify-center transition-none"
                >
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("checking")}
                </Button>
              ) : availableUpdate ? (
                isDesktop() ? (
                  <Button
                    size="sm"
                    onClick={installUpdate}
                    disabled={installingUpdate}
                  >
                    {installingUpdate ? (
                      <>
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        {t("updating")}
                      </>
                    ) : (
                      <>
                        <ArrowUpCircle className="h-3.5 w-3.5" />
                        {t("upgradeTo", { version: availableUpdate.version })}
                      </>
                    )}
                  </Button>
                ) : serverSelfUpdate ? (
                  <Button
                    size="sm"
                    onClick={runServerUpgrade}
                    disabled={installingUpdate}
                  >
                    {installingUpdate ? (
                      <>
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        {t("updating")}
                      </>
                    ) : (
                      <>
                        <ArrowUpCircle className="h-3.5 w-3.5" />
                        {t("upgradeTo", { version: availableUpdate.version })}
                      </>
                    )}
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    onClick={() =>
                      openUrl(
                        "https://github.com/xintaofei/codeg/releases/latest"
                      )
                    }
                  >
                    <ArrowUpCircle className="h-3.5 w-3.5" />
                    {t("viewRelease", { version: availableUpdate.version })}
                  </Button>
                )
              ) : (
                <Button
                  key="check-update"
                  size="sm"
                  onClick={checkForUpdates}
                  disabled={installingUpdate}
                  className="w-[9.5rem] justify-center transition-none"
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("checkUpdate")}
                </Button>
              )}
            </div>

            {!availableUpdate && formattedLastCheckedAt && (
              <p className="text-muted-foreground">
                {t("lastChecked", { time: formattedLastCheckedAt })}
              </p>
            )}

            {updateStatusMessage && !downloadProgress && (
              <p className="text-muted-foreground">{updateStatusMessage}</p>
            )}

            {downloadProgress && (
              <div className="space-y-1.5">
                <div className="flex items-center justify-between text-muted-foreground">
                  <span>
                    {downloadProgress.phase === "downloading"
                      ? t("downloading")
                      : t("updating")}
                  </span>
                  <span>
                    {formatBytes(downloadProgress.downloaded)}
                    {downloadProgress.total
                      ? ` / ${formatBytes(downloadProgress.total)}`
                      : ""}
                  </span>
                </div>
                <div className="h-1.5 rounded-full bg-muted overflow-hidden">
                  <div
                    className="h-full rounded-full bg-primary transition-all duration-300"
                    style={{
                      width:
                        downloadProgress.total && downloadProgress.total > 0
                          ? `${Math.min(100, (downloadProgress.downloaded / downloadProgress.total) * 100)}%`
                          : "30%",
                    }}
                  />
                </div>
              </div>
            )}

            {restartCountdown !== null && (
              <p className="flex items-center gap-2 text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                {restartCountdown > 0
                  ? t("restartingIn", { seconds: restartCountdown })
                  : t("waitingForServer")}
              </p>
            )}

            {availableUpdate &&
              serverSelfUpdate &&
              serverRuntime === "docker" && (
                <p className="text-muted-foreground/80 text-[11px] leading-5">
                  {t("dockerUpgradeHint")}
                </p>
              )}

            {availableUpdate && (
              <div className="space-y-2 pt-2 border-t border-border/70">
                <div className="flex items-center justify-between gap-3">
                  <span className="font-medium">
                    {t("upgradableVersion")}：v{availableUpdate.version}
                  </span>
                  {formattedUpdateDate && (
                    <span className="text-muted-foreground text-[11px]">
                      {formattedUpdateDate}
                    </span>
                  )}
                </div>
                <div
                  className={
                    "mt-3 max-h-72 overflow-auto rounded-md border bg-background/70 px-3 py-3 leading-6 break-words text-muted-foreground " +
                    "[&_h1]:text-sm [&_h1]:font-semibold [&_h1]:mb-2 [&_h1]:text-foreground " +
                    "[&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-2 [&_h2]:text-foreground " +
                    "[&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 [&_h3]:text-foreground " +
                    "[&_p]:mb-2 [&_p:last-child]:mb-0 " +
                    "[&_ul]:list-disc [&_ul]:pl-5 [&_ul]:mb-2 [&_ol]:list-decimal [&_ol]:pl-5 [&_ol]:mb-2 [&_li]:mb-1 " +
                    "[&_code]:font-mono [&_code]:text-[11px] [&_code]:bg-muted [&_code]:rounded [&_code]:px-1 " +
                    "[&_pre]:bg-muted [&_pre]:rounded-md [&_pre]:p-2 [&_pre]:overflow-x-auto [&_pre]:mb-2 " +
                    "[&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2 " +
                    "[&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground/80 " +
                    "[&_hr]:my-2 [&_hr]:border-border"
                  }
                >
                  {updateNotes ? (
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>
                      {updateNotes}
                    </ReactMarkdown>
                  ) : (
                    t("none")
                  )}
                </div>
              </div>
            )}
          </div>

          {updateError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {t("updateError", { message: updateError })}
            </div>
          )}
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Wifi className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("proxyTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("proxyDescription")}
          </p>

          {loadError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {t("loadFailed", { message: loadError })}
            </div>
          )}

          <label className="inline-flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={enabled}
              disabled={saving}
              onChange={(event) => {
                const next = event.target.checked
                if (next && !proxyUrl.trim()) {
                  setProxyUrlError(t("proxyRequired"))
                  return
                }
                setProxyUrlError(null)
                setEnabled(next)
                saveProxySettings(next, proxyUrl)
              }}
            />
            {t("enableProxy")}
          </label>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("proxyAddress")}
            </label>
            <Input
              value={proxyUrl}
              onChange={(event) => {
                setProxyUrl(event.target.value)
                if (event.target.value.trim()) setProxyUrlError(null)
              }}
              onBlur={() => {
                if (enabled && !proxyUrl.trim()) {
                  setProxyUrlError(t("proxyRequired"))
                  return
                }
                setProxyUrlError(null)
                saveProxySettings(enabled, proxyUrl)
              }}
              placeholder={PROXY_EXAMPLE}
              disabled={saving}
              aria-invalid={proxyUrlError ? true : undefined}
            />
            {proxyUrlError && (
              <p className="text-[11px] text-destructive">{proxyUrlError}</p>
            )}
            <p className="text-[11px] text-muted-foreground">
              {t("proxyHint", { example: PROXY_EXAMPLE })}
            </p>
          </div>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Languages className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("languageTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("languageDescription")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("appLanguage")}
            </label>
            <Select
              value={appLanguage}
              onValueChange={(value) => {
                let nextLang: LanguageSelectValue
                if (value === "system") {
                  nextLang = "system"
                } else if (isAppLocale(value)) {
                  nextLang = value
                } else {
                  return
                }
                setAppLanguage(nextLang)
                saveLanguage(nextLang)
              }}
              disabled={savingLanguage || !languageSettingsLoaded}
            >
              <SelectTrigger className="w-full sm:w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="system">
                  {tLanguage("followSystem")}
                </SelectItem>
                <SelectItem value="en">{languageLabels.en}</SelectItem>
                <SelectItem value="zh_cn">{languageLabels.zh_cn}</SelectItem>
                <SelectItem value="zh_tw">{languageLabels.zh_tw}</SelectItem>
                <SelectItem value="ja">{languageLabels.ja}</SelectItem>
                <SelectItem value="ko">{languageLabels.ko}</SelectItem>
                <SelectItem value="es">{languageLabels.es}</SelectItem>
                <SelectItem value="de">{languageLabels.de}</SelectItem>
                <SelectItem value="fr">{languageLabels.fr}</SelectItem>
                <SelectItem value="pt">{languageLabels.pt}</SelectItem>
                <SelectItem value="ar">{languageLabels.ar}</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </section>
      </div>
    </ScrollArea>
  )
}
