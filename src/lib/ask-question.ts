/**
 * Parsing helpers shared by the live `AskQuestionCard` (interactive) and the
 * historical `AskQuestionResultCard` (read-only, in the message stream).
 *
 * The codeg-mcp `ask_user_question` tool serializes into a session transcript
 * as a generic tool call. The input is the raw `{ questions: [...] }` JSON the
 * agent sent. The output is the tool result the agent CLI persisted: in
 * practice that is the companion's structured `{ answers, declined }` envelope
 * (each answer's `selected` is already a string array — see `render_ask_result`
 * in `src-tauri/src/acp/delegation/companion.rs`), which is what we parse first.
 * We also fall back to the companion's human-readable result text for any CLI
 * that persists `content` instead of `structuredContent`.
 */

export interface AskQuestionOption {
  label: string
  description: string
}

export interface AskQuestion {
  question: string
  header: string
  /** The wire field is `multiSelect` (camelCase); we also accept `multi_select`. */
  multiSelect: boolean
  options: AskQuestionOption[]
}

export interface AskQuestionAnswer {
  header: string
  question: string
  /** The user's raw picks: each entry is one offered option label or a free-text
   *  "Other" answer (empty when nothing was chosen). Partition against the
   *  question's options with `matchSelections`. */
  selected: string[]
}

export interface AskQuestionOutcome {
  declined: boolean
  answers: AskQuestionAnswer[]
}

/**
 * Strip a trailing " (Recommended)" so it can render as a badge while the
 * underlying value keeps the agent's original label verbatim. Shared so the
 * live and historical cards present recommendations identically.
 */
export function splitRecommended(label: string): {
  text: string
  recommended: boolean
} {
  const m = label.match(/^(.*?)\s*\(recommended\)\s*$/i)
  const text = m?.[1].trim()
  // Only treat "(Recommended)" as a suffix when real text precedes it — a bare
  // "(Recommended)" label keeps its literal text rather than rendering empty.
  return text
    ? { text, recommended: true }
    : { text: label, recommended: false }
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : ""
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null
}

function parseOptions(raw: unknown): AskQuestionOption[] {
  if (!Array.isArray(raw)) return []
  const out: AskQuestionOption[] = []
  for (const item of raw) {
    if (!item || typeof item !== "object") continue
    const obj = item as Record<string, unknown>
    const label = asString(obj.label)
    // An option with no label carries no meaning to display; drop it.
    if (!label) continue
    out.push({ label, description: asString(obj.description) })
  }
  return out
}

/**
 * Parse the `ask_user_question` tool input (the raw `{ questions: [...] }` JSON
 * the agent sent). Tolerant of partial/streaming input and missing fields —
 * returns `[]` rather than throwing so callers can fall back gracefully.
 */
export function parseAskQuestionInput(
  input: string | null | undefined
): AskQuestion[] {
  if (!input) return []
  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    return []
  }
  if (!parsed || typeof parsed !== "object") return []
  const questions = (parsed as Record<string, unknown>).questions
  if (!Array.isArray(questions)) return []

  const out: AskQuestion[] = []
  for (const item of questions) {
    if (!item || typeof item !== "object") continue
    const obj = item as Record<string, unknown>
    const options = parseOptions(obj.options)
    const question = asString(obj.question)
    // An entry with neither prompt text nor options is empty noise; skip it.
    if (!question && options.length === 0) continue
    out.push({
      question,
      header: asString(obj.header),
      multiSelect: obj.multiSelect === true || obj.multi_select === true,
      options,
    })
  }
  return out
}

/** The companion's marker for an answered-but-empty selection (English, not localized). */
const NO_SELECTION = "(no selection)"
const HEADER_LINE_RE = /^\s*\d+\.\s*\[([^\]]*)\]\s*(.*)$/
const SELECTED_LINE_RE = /^\s*→\s*(.*)$/

function parseAnswers(raw: unknown): AskQuestionAnswer[] {
  if (!Array.isArray(raw)) return []
  const out: AskQuestionAnswer[] = []
  for (const item of raw) {
    const obj = asRecord(item)
    if (!obj) continue
    const selected = Array.isArray(obj.selected)
      ? obj.selected.filter((x): x is string => typeof x === "string")
      : []
    out.push({
      header: asString(obj.header),
      question: asString(obj.question),
      selected,
    })
  }
  return out
}

/**
 * Parse the structured `{ answers, declined }` envelope the agent CLI persists
 * for the tool result (the companion's `structuredContent`). It may sit at the
 * top level or nested under `structuredContent`. Returns `null` when `output`
 * is not that envelope, so the text fallback can take over.
 */
function parseOutcomeJson(output: string): AskQuestionOutcome | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(output)
  } catch {
    return null
  }
  const top = asRecord(parsed)
  if (!top) return null
  const env =
    Array.isArray(top.answers) || typeof top.declined === "boolean"
      ? top
      : asRecord(top.structuredContent)
  if (!env) return null
  if (!Array.isArray(env.answers) && typeof env.declined !== "boolean") {
    return null
  }
  if (env.declined === true) return { declined: true, answers: [] }
  return { declined: false, answers: parseAnswers(env.answers) }
}

/**
 * Reconstruct the answered/declined outcome from the persisted tool result.
 *
 * Primary shape — the structured envelope the CLI stores verbatim:
 *   {"answers":[{"header":"…","question":"…","selected":["A","B"]}],"declined":false}
 *
 * Fallback shape — the companion's human-readable text, for any CLI that keeps
 * `content` instead of `structuredContent` (see `render_ask_result`):
 *   "The user dismissed the question(s) …"  (declined)
 *   "The user answered your question(s):\n1. [Header] Question\n   → a, b\n…"
 *
 * Returns `null` when there is no output yet (the call is still in flight). In
 * the fallback, selections are split on ", " (lossy for a label containing a
 * comma — the structured envelope keeps such a label intact as one array entry).
 */
export function parseAskQuestionOutcome(
  output: string | null | undefined
): AskQuestionOutcome | null {
  if (!output || !output.trim()) return null

  const fromJson = parseOutcomeJson(output)
  if (fromJson) return fromJson

  if (/\bdismissed the question/i.test(output)) {
    return { declined: true, answers: [] }
  }

  const answers: AskQuestionAnswer[] = []
  let current: AskQuestionAnswer | null = null
  for (const line of output.split(/\r?\n/)) {
    const header = line.match(HEADER_LINE_RE)
    if (header) {
      current = {
        header: header[1].trim(),
        question: header[2].trim(),
        selected: [],
      }
      answers.push(current)
      continue
    }
    const selectedLine = line.match(SELECTED_LINE_RE)
    if (selectedLine && current) {
      const joined = selectedLine[1].trim()
      current.selected =
        joined && joined !== NO_SELECTION ? joined.split(", ") : []
      current = null
    }
  }
  return { declined: false, answers }
}

/**
 * Partition the user's raw picks into the offered option labels they chose
 * (`selected`) and any free-text "Other" answers (`other`), order-preserving.
 * Each pick is already a whole value (one array entry), so a label that itself
 * contains a comma matches cleanly — no fragile text splitting required.
 */
export function matchSelections(
  values: string[],
  optionLabels: string[]
): { selected: string[]; other: string[] } {
  const labels = new Set(optionLabels.filter(Boolean))
  const selected: string[] = []
  const other: string[] = []
  for (const raw of values) {
    const value = raw.trim()
    if (!value || value === NO_SELECTION) continue
    if (labels.has(value)) selected.push(value)
    else other.push(value)
  }
  return { selected, other }
}
