"use client"

import { useSyncExternalStore } from "react"
import { Home } from "lucide-react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { isDesktop, returnHome } from "@/lib/platform"

function subscribeRuntime() {
  return () => {}
}

export function useShowReturnHomeButton() {
  return useSyncExternalStore(
    subscribeRuntime,
    () => !isDesktop(),
    () => false
  )
}

export function ReturnHomeButton() {
  const t = useTranslations("SettingsShell")

  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      className="h-7 gap-1.5 px-2 text-xs"
      onClick={returnHome}
      aria-label={t("returnHome")}
      title={t("returnHome")}
    >
      <Home className="h-3.5 w-3.5" />
      <span>{t("returnHome")}</span>
    </Button>
  )
}
