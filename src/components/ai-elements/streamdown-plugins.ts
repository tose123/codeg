"use client"

import { useEffect, useMemo, useSyncExternalStore } from "react"
import type { ComponentProps } from "react"
import { cjk } from "@streamdown/cjk"
import type { Streamdown } from "streamdown"

// Streamdown's `plugins` prop config: `{ code?, mermaid?, math?, cjk? }`.
type PluginConfig = NonNullable<ComponentProps<typeof Streamdown>["plugins"]>
type CodePlugin = NonNullable<PluginConfig["code"]>
type MathPlugin = NonNullable<PluginConfig["math"]>
type MermaidPlugin = NonNullable<PluginConfig["mermaid"]>
type HeavyKind = "code" | "math" | "mermaid"

// --- Why this module exists --------------------------------------------------
//
// The three heavy Streamdown plugins pull in the largest dependencies in the
// whole frontend: `@streamdown/code` → shiki (+ its full grammar/theme index,
// ~3.8M), `@streamdown/math` → katex (~4.3M), `@streamdown/mermaid` → mermaid +
// cytoscape + the entire d3 galaxy + dagre (~70M+ unpacked). Statically
// importing them — as `message.tsx`, `reasoning.tsx`, and `file-workspace-panel`
// each did — forces all three (and their transitive deps) into the first-paint
// chunk of `/workspace`, so even a text-only conversation downloads, parses, and
// executes them. Streamdown itself already `React.lazy`-loads its *render*
// components (code-block, mermaid) internally; the eager static plugin imports
// defeated that by pinning the heavy *engines* eagerly.
//
// This module loads each engine on demand — the first time rendered content
// actually contains the syntax that plugin handles — and at most once
// process-wide. Mounted consumers subscribe to a version counter and re-render
// when an engine resolves, upgrading the already-rendered fallback in place.
//
// Correctness: each engine only *affects* output when its trigger syntax is
// present (shiki only highlights fenced code; remark-math only transforms `$…$`;
// mermaid only replaces ```mermaid blocks). Loading exactly when the trigger
// appears therefore reproduces the eager-plugin output byte-for-byte, save for a
// one-time pre-load fallback render (plain code / literal `$…$` / mermaid source)
// that upgrades once the engine arrives. Detection errs LOOSE on purpose — a
// false positive merely pre-loads an engine that then no-ops, exactly matching
// the previous always-loaded behavior; a false *negative* would silently drop
// real rendering, so the math trigger is a superset (`$` OR the pre-normalized
// `\[` / `\(` delimiters that `normalizeMathDelimiters` maps to `$`).

const loaded: {
  code?: CodePlugin
  math?: MathPlugin
  mermaid?: MermaidPlugin
} = {}
const inflight = new Set<HeavyKind>()
const listeners = new Set<() => void>()
let version = 0

function emit(): void {
  version += 1
  for (const listener of listeners) listener()
}

function subscribe(onStoreChange: () => void): () => void {
  listeners.add(onStoreChange)
  return () => {
    listeners.delete(onStoreChange)
  }
}

function getVersion(): number {
  return version
}

// Guard against unsupported language identifiers (e.g. "##", "function", or a
// bare "mermaid" while its engine is still loading) that appear as the info
// string of fenced code blocks from tool output. Without this, Shiki's
// createHighlighter tries to load unknown grammars and produces noisy console
// errors. Verbatim behavior from the former `message.tsx` `safeCode`.
//
// NOTE: this wrapper is now applied uniformly to ALL Streamdown consumers.
// `message.tsx` previously wrapped `code`; `reasoning.tsx` and the markdown file
// preview passed the raw plugin. Applying the guard everywhere is deliberate:
// the rendered TEXT/semantic content is unchanged — an unsupported fence
// language stays plaintext either way — and it suppresses Shiki's unknown-
// grammar console noise for those two consumers too. (Styling may differ
// marginally: raw Shiki returns null for an unknown grammar and falls back to
// un-themed text, whereas mapping to "text" yields themed plaintext tokens —
// this only makes code-block theming more consistent, never changes content.)
function makeSafeCode(code: CodePlugin): CodePlugin {
  return {
    ...code,
    highlight(options, callback) {
      const language = code.supportsLanguage(options.language)
        ? options.language
        : ("text" as typeof options.language)
      return code.highlight({ ...options, language }, callback)
    },
  }
}

function ensure(kind: HeavyKind): void {
  if (loaded[kind] || inflight.has(kind)) return
  inflight.add(kind)
  const settle = () => {
    inflight.delete(kind)
    emit()
  }
  if (kind === "code") {
    import("@streamdown/code")
      .then((mod) => {
        loaded.code = makeSafeCode(mod.code)
      })
      .catch(() => {})
      .finally(settle)
  } else if (kind === "math") {
    import("@streamdown/math")
      .then((mod) => {
        loaded.math = mod.createMathPlugin({ singleDollarTextMath: true })
      })
      .catch(() => {})
      .finally(settle)
  } else {
    import("@streamdown/mermaid")
      .then((mod) => {
        loaded.mermaid = mod.mermaid
      })
      .catch(() => {})
      .finally(settle)
  }
}

export type HeavyPluginNeeds = Record<HeavyKind, boolean>

const NO_NEEDS: HeavyPluginNeeds = { code: false, math: false, mermaid: false }

/**
 * Cheap, behavior-preserving detection of which heavy plugins a piece of
 * Markdown needs. Runs on possibly-streaming text (once per streaming batch for
 * the live message; memoized to nothing for stable historical text), so it uses
 * only native `String` scans.
 */
export function detectHeavyPlugins(text: string): HeavyPluginNeeds {
  const hasFence = text.includes("```") || text.includes("~~~")
  // CommonMark also renders indented blocks (≥4 spaces or a tab) as
  // `<pre><code>`, and the old eager plugin highlighted those too. Detecting
  // them precisely is context-sensitive — the required indent shifts inside
  // blockquotes (`>     x`), list items, and nested containers — so instead of
  // emulating a parser we use a GUARANTEED superset: every indented code line
  // has a run of ≥4 consecutive spaces or a tab in its RAW text (container
  // markers only ADD indent, never remove it), so we look for that run
  // anywhere. This can over-load the (small) Shiki engine for prose that merely
  // contains a tab or aligned spacing — safe, since it only reproduces the old
  // always-loaded behavior, the load is one-time per session, and the
  // per-language grammars (plus mermaid/katex) stay lazy regardless. Missing a
  // real indented block would instead leave it permanently unhighlighted (a
  // regression), so we err to the superset.
  const hasIndentedCode = / {4}|\t/.test(text)
  return {
    // Any fenced or indented block may want syntax highlighting.
    code: hasFence || hasIndentedCode,
    // `$` is remark-math's only delimiter; `normalizeMathDelimiters` rewrites
    // `\[…\]` / `\(…\)` to `$$…$$` / `$…$`, but a caller may detect on the raw
    // pre-normalized text, so treat those escapes as math triggers too. No such
    // token ⇒ remark-math is a no-op ⇒ katex is not needed.
    math: text.includes("$") || text.includes("\\[") || text.includes("\\("),
    // A ```mermaid (or ~~~mermaid) fence is the only thing the diagram engine
    // renders.
    mermaid: /(?:```|~~~)[^\S\r\n]*mermaid\b/i.test(text),
  }
}

const CJK_ONLY: PluginConfig = { cjk }

/**
 * Returns the Streamdown `plugins` config for `text`, loading the heavy engines
 * lazily and only when `text` needs them. Pass `null`/`undefined` (e.g. for
 * non-string children, or a non-preview mode) to request the light config only.
 */
export function useStreamdownPlugins(
  text: string | null | undefined
): PluginConfig {
  const needs = useMemo(
    () => (typeof text === "string" ? detectHeavyPlugins(text) : NO_NEEDS),
    [text]
  )
  // Destructure to primitives so the effect and the returned `plugins` object
  // stay reference-stable across streaming batches. `needs` is a fresh object
  // every time `text` grows (~60/s for the live message), but its booleans
  // rarely change; keying the effect and memo on the booleans avoids re-running
  // the effect and — crucially — avoids handing Streamdown a new `plugins`
  // object on every token, which would churn its plugin-dependent memos.
  const { code: needCode, math: needMath, mermaid: needMermaid } = needs

  // Re-render this consumer whenever a lazily-imported engine resolves.
  const currentVersion = useSyncExternalStore(subscribe, getVersion, getVersion)

  useEffect(() => {
    if (needCode) ensure("code")
    if (needMath) ensure("math")
    if (needMermaid) ensure("mermaid")
  }, [needCode, needMath, needMermaid])

  return useMemo(() => {
    if (!needCode && !needMath && !needMermaid) return CJK_ONLY
    const plugins: PluginConfig = { cjk }
    if (needCode && loaded.code) plugins.code = loaded.code
    if (needMath && loaded.math) plugins.math = loaded.math
    if (needMermaid && loaded.mermaid) plugins.mermaid = loaded.mermaid
    return plugins
    // `currentVersion` is the load signal: a resolved `ensure()` mutates the
    // module `loaded` cache and bumps the version, so this memo re-runs and
    // splices the freshly-loaded engine in. The needs booleans are the other
    // deps; `loaded` itself is intentionally read untracked (version covers it).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [needCode, needMath, needMermaid, currentVersion])
}

/** Test-only: reset the module-level plugin cache between test cases. */
export function __resetStreamdownPluginsForTest(): void {
  loaded.code = undefined
  loaded.math = undefined
  loaded.mermaid = undefined
  inflight.clear()
  listeners.clear()
  version = 0
}
