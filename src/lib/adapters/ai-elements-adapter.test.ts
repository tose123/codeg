import { describe, expect, it } from "vitest"

import {
  adaptMessageTurn,
  createMessageTurnAdapter,
  dropHiddenFeedbackChecks,
  extractUserResourcesFromText,
  groupConsecutiveDelegationStatus,
  groupGoalRuns,
  groupConsecutiveToolCalls,
  mergeAdjacentDelegationStatusGroups,
  type AdaptedContentPart,
  type AdaptedToolCallPart,
} from "./ai-elements-adapter"

function poll(toolName: string, taskId?: string): AdaptedToolCallPart {
  return {
    type: "tool-call",
    toolCallId: `${toolName}:${taskId ?? ""}`,
    toolName,
    input: taskId ? JSON.stringify({ task_id: taskId }) : null,
    state: "output-available",
  }
}

const text: AdaptedContentPart = { type: "text", text: "checking again" }

function pollsOf(part: AdaptedContentPart): AdaptedToolCallPart[] {
  if (part.type !== "delegation-status-group") {
    throw new Error(`expected a delegation-status-group, got ${part.type}`)
  }
  return part.polls
}

function goalRunOf(part: AdaptedContentPart) {
  if (part.type !== "goal-run") {
    throw new Error(`expected a goal-run, got ${part.type}`)
  }
  return part
}

describe("groupConsecutiveDelegationStatus", () => {
  it("wraps a run of consecutive status polls into one group", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("wraps even a single poll (so the settled-status rule applies uniformly)", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(1)
  })

  it("groups interleaved parallel polls together (consecutive run)", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("get_delegation_status", "t2"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("does NOT merge polls separated by text", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      text,
      poll("get_delegation_status", "t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "text",
      "delegation-status-group",
    ])
  })

  it("breaks the run on delegate_to_agent and cancel_delegation", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("get_delegation_status", "t1"),
      poll("delegate_to_agent", "t2"),
      poll("get_delegation_status", "t1"),
      poll("cancel_delegation", "t1"),
      poll("get_delegation_status", "t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "tool-call",
      "delegation-status-group",
      "tool-call",
      "delegation-status-group",
    ])
  })

  it("matches host-prefixed historical names", () => {
    const out = groupConsecutiveDelegationStatus([
      poll("mcp__codeg-mcp__get_delegation_status", "t1"),
      poll("mcp__codeg-delegate__get_delegation_status", "t1"),
      poll("codeg-delegate/get_delegation_status", "t1"),
    ])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(3)
  })

  it("leaves a non-status part untouched", () => {
    const toolGroup: AdaptedContentPart = {
      type: "tool-group",
      items: [],
      isStreaming: false,
    }
    expect(groupConsecutiveDelegationStatus([toolGroup])).toEqual([toolGroup])
  })
})

describe("groupConsecutiveToolCalls", () => {
  it("leaves Codex goal calls standalone so they can render as cards", () => {
    const out = groupConsecutiveToolCalls([
      poll("create_goal"),
      poll("exec_command"),
      poll("update_goal"),
    ])

    expect(out.map((p) => p.type)).toEqual([
      "tool-call",
      "tool-group",
      "tool-call",
    ])
  })
})

describe("dropHiddenFeedbackChecks", () => {
  const FEEDBACK_OUT =
    'Wall time: 0.003 seconds\nOutput:\n{"count":1,"feedback":[{"created_at":"2026-06-09T07:47:12Z","text":"还有package"}]}'
  const NO_FEEDBACK_OUT =
    'Wall time: 0.002 seconds\nOutput:\n{"count":0,"feedback":[]}'

  function feedbackCheck(
    output: string | null,
    extra: Partial<AdaptedToolCallPart> = {}
  ): AdaptedToolCallPart {
    return {
      type: "tool-call",
      toolCallId: `cuf:${output ?? "pending"}`,
      toolName: "check_user_feedback",
      input: "{}",
      state: output ? "output-available" : "input-available",
      output,
      ...extra,
    }
  }

  it("drops no-feedback, in-flight, and unparseable checks", () => {
    const out = dropHiddenFeedbackChecks([
      feedbackCheck(NO_FEEDBACK_OUT),
      feedbackCheck(null),
      feedbackCheck("some unrelated output"),
    ])
    expect(out).toHaveLength(0)
  })

  it("keeps checks that received feedback", () => {
    const part = feedbackCheck(FEEDBACK_OUT)
    expect(dropHiddenFeedbackChecks([part])).toEqual([part])
  })

  it("keeps errored checks so failures aren't swallowed", () => {
    const errored = feedbackCheck(null, {
      state: "output-error",
      errorText: "boom",
    })
    expect(dropHiddenFeedbackChecks([errored])).toEqual([errored])
  })

  it("never touches non-feedback parts", () => {
    const parts: AdaptedContentPart[] = [
      poll("exec_command"),
      text,
      poll("read"),
    ]
    expect(dropHiddenFeedbackChecks(parts)).toEqual(parts)
  })

  it("collapses neighbours into one group once a no-op check is dropped", () => {
    const grouped = groupConsecutiveToolCalls(
      dropHiddenFeedbackChecks([
        poll("exec_command"),
        feedbackCheck(NO_FEEDBACK_OUT),
        poll("read"),
      ])
    )
    // Without the drop, the standalone check would split this into two groups.
    expect(grouped.map((p) => p.type)).toEqual(["tool-group"])
  })

  it("breaks the run when a check carries feedback", () => {
    const grouped = groupConsecutiveToolCalls(
      dropHiddenFeedbackChecks([
        poll("exec_command"),
        feedbackCheck(FEEDBACK_OUT),
        poll("read"),
      ])
    )
    expect(grouped.map((p) => p.type)).toEqual([
      "tool-group",
      "tool-call",
      "tool-group",
    ])
  })
})

describe("groupGoalRuns", () => {
  it("wraps create_goal through update_goal with intervening process parts", () => {
    const grouped = groupConsecutiveToolCalls([
      poll("create_goal"),
      text,
      poll("exec_command"),
      poll("update_goal"),
      { type: "text", text: "final answer" },
    ])

    const out = groupGoalRuns(grouped)

    expect(out.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(out[0])
    expect(goalRun.start.toolName).toBe("create_goal")
    expect(goalRun.end?.toolName).toBe("update_goal")
    expect(goalRun.items.map((p) => p.type)).toEqual(["text", "tool-group"])
    expect(goalRun.isRunning).toBe(false)
  })

  it("wraps an unfinished goal run as running", () => {
    const out = groupGoalRuns([poll("create_goal"), text])

    expect(out).toHaveLength(1)
    const goalRun = goalRunOf(out[0])
    expect(goalRun.end).toBeNull()
    expect(goalRun.items).toEqual([text])
    expect(goalRun.isRunning).toBe(true)
  })

  it("does not mutate a reopened unfinished goal run when closing across turns", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const nextText: AdaptedContentPart = {
      type: "text",
      text: "continued goal",
    }
    const unfinished: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }

    const firstMerge = groupGoalRuns([
      unfinished,
      nextText,
      poll("update_goal"),
    ])
    expect(goalRunOf(firstMerge[0]).items).toEqual([firstText, nextText])
    expect(goalRunOf(unfinished).items).toEqual([firstText])

    const secondMerge = groupGoalRuns([
      unfinished,
      nextText,
      poll("update_goal"),
    ])
    expect(goalRunOf(secondMerge[0]).items).toEqual([firstText, nextText])
  })

  it("merges repeated unfinished goal runs into one cross-turn card", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const nextText: AdaptedContentPart = {
      type: "text",
      text: "continued goal",
    }
    const firstRun: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }
    const repeatedRun: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [],
      isRunning: true,
    }

    const out = groupGoalRuns([firstRun, repeatedRun, nextText])

    expect(out).toHaveLength(1)
    expect(goalRunOf(out[0]).items).toEqual([firstText, nextText])
  })

  it("closes an active cross-turn goal when the next turn already has a completed goal run", () => {
    const firstText: AdaptedContentPart = {
      type: "text",
      text: "started goal",
    }
    const toolGroup: AdaptedContentPart = {
      type: "tool-group",
      items: [poll("exec_command")],
      isStreaming: false,
    }
    const finalText: AdaptedContentPart = {
      type: "text",
      text: "final answer",
    }
    const unfinished: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: null,
      items: [firstText],
      isRunning: true,
    }
    const completed: AdaptedContentPart = {
      type: "goal-run",
      start: poll("create_goal"),
      end: poll("update_goal"),
      items: [toolGroup],
      isRunning: false,
    }

    const out = groupGoalRuns([unfinished, completed, finalText])

    expect(out.map((p) => p.type)).toEqual(["goal-run", "text"])
    expect(goalRunOf(out[0]).items).toEqual([firstText, toolGroup])
    expect(out[1]).toEqual(finalText)
  })
})

describe("adaptMessageTurn goal update text", () => {
  it("converts streaming Codex goal update text into a running goal card", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "我会先建立这个目标。\nGoal updated (active): 分析 README 文件\n",
          },
          {
            type: "tool_use",
            tool_use_id: "exec-1",
            tool_name: "exec_command",
            input_preview: JSON.stringify({ cmd: "sed -n '1,120p' README.md" }),
          },
          {
            type: "tool_result",
            tool_use_id: "exec-1",
            output_preview: "README content",
            is_error: false,
          },
          {
            type: "text",
            text: "Goal updated (active): 分析 README 文件\n",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["text", "goal-run"])
    expect(adapted.content[0]).toEqual({
      type: "text",
      text: "我会先建立这个目标。",
    })
    const goalRun = goalRunOf(adapted.content[1])
    expect(goalRun.start.toolName).toBe("create_goal")
    expect(goalRun.end).toBeNull()
    expect(goalRun.isRunning).toBe(true)
    expect(goalRun.items.map((p) => p.type)).toEqual(["tool-group"])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
  })

  it("keeps final text outside a completed goal when a stale active update arrives after completion", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-complete",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "Goal updated (active): 分析 README 文件\n",
          },
          {
            type: "tool_use",
            tool_use_id: "exec-1",
            tool_name: "exec_command",
            input_preview: JSON.stringify({ cmd: "sed -n '1,120p' README.md" }),
          },
          {
            type: "tool_result",
            tool_use_id: "exec-1",
            output_preview: "README content",
            is_error: false,
          },
          {
            type: "text",
            text:
              "Goal updated (complete): 分析 README 文件\n" +
              "Goal updated (active): 分析 README 文件\n" +
              "已完成 README 分析。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(goalRun.end?.toolName).toBe("update_goal")
    expect(goalRun.isRunning).toBe(false)
    expect(adapted.content[1]).toEqual({
      type: "text",
      text: "已完成 README 分析。",
    })
  })

  it("does not absorb unseparated prose and later goal markers into the objective", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-concatenated",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text:
              "Goal updated (active): 分析 README 文件" +
              "我也顺手对照了 `package.json` 和 `app` 目录。" +
              "Goal updated (active): 分析 README 文件" +
              "Goal updated (complete): 分析 README 文件" +
              "已分析 [README.md](/Users/xggz/my/my-app/README.md:1)。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run", "text"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(JSON.parse(goalRun.end?.output ?? "{}")).toMatchObject({
      goal: {
        objective: "分析 README 文件",
        status: "complete",
      },
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
    expect(adapted.content[1]).toEqual({
      type: "text",
      text: "已分析 [README.md](/Users/xggz/my/my-app/README.md:1)。",
    })
  })

  it("keeps the known streaming objective when later text is appended without a separator", () => {
    const adapter = createMessageTurnAdapter()
    const textLabels = {
      attachedResources: "Attached resources",
      toolCallFailed: "Tool failed",
    }
    const firstTurn = {
      id: "live-turn-single-marker",
      role: "assistant" as const,
      timestamp: "2026-06-02T00:00:00.000Z",
      blocks: [
        {
          type: "text" as const,
          text: "Goal updated (active): 分析 README 文件",
        },
      ],
    }
    const secondTurn = {
      ...firstTurn,
      blocks: [
        {
          type: "text" as const,
          text:
            "Goal updated (active): 分析 README 文件" +
            "我也顺手对照了 `package.json` 和 `app` 目录。",
        },
      ],
    }

    adapter.adapt([firstTurn], textLabels, new Set([0]))
    const [adapted] = adapter.adapt([secondTurn], textLabels, new Set([0]))

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
  })

  it("does not absorb adjacent Chinese prose into a single active marker objective", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-turn-single-marker-prose",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text:
              "Goal updated (active): 分析 README 文件" +
              "我也顺手对照了 `package.json` 和 `app` 目录。",
          },
        ],
      },
      {
        attachedResources: "Attached resources",
        toolCallFailed: "Tool failed",
      },
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["goal-run"])
    const goalRun = goalRunOf(adapted.content[0])
    expect(JSON.parse(goalRun.start.input ?? "{}")).toEqual({
      objective: "分析 README 文件",
    })
    expect(goalRun.items).toEqual([
      {
        type: "text",
        text: "我也顺手对照了 `package.json` 和 `app` 目录。",
      },
    ])
  })
})

describe("mergeAdjacentDelegationStatusGroups", () => {
  const group = (taskId: string): AdaptedContentPart => ({
    type: "delegation-status-group",
    polls: [poll("get_delegation_status", taskId)],
  })

  it("merges adjacent groups (cross-turn concatenation)", () => {
    const out = mergeAdjacentDelegationStatusGroups([group("t1"), group("t1")])
    expect(out).toHaveLength(1)
    expect(pollsOf(out[0])).toHaveLength(2)
  })

  it("does not merge groups separated by another part", () => {
    const out = mergeAdjacentDelegationStatusGroups([
      group("t1"),
      text,
      group("t1"),
    ])
    expect(out.map((p) => p.type)).toEqual([
      "delegation-status-group",
      "text",
      "delegation-status-group",
    ])
  })
})

describe("adaptMessageTurn plan handling", () => {
  const msgText = {
    attachedResources: "Attached resources",
    toolCallFailed: "Tool failed",
  }

  it("renders a live synthetic plan block as a plan part (not reasoning) and marks the last block streaming", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-plan",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "plan",
            entries: [
              { content: "Step A", status: "in_progress", priority: "high" },
              { content: "Step B", status: "completed", priority: "low" },
            ],
          },
        ],
      },
      msgText,
      true
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["plan"])
    const plan = adapted.content[0]
    if (plan.type !== "plan") throw new Error("expected a plan part")
    expect(plan.isStreaming).toBe(true)
    expect(plan.entries).toEqual([
      { content: "Step A", status: "in_progress", priority: "high" },
      { content: "Step B", status: "completed", priority: "low" },
    ])
  })

  it("converts a persisted TodoWrite tool_use (+ its result) into a single plan part with no orphan tool-result", () => {
    const adapted = adaptMessageTurn(
      {
        id: "hist-plan",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: JSON.stringify({
              todos: [
                { content: "X", status: "pending", priority: "medium" },
                { content: "Y", status: "completed", priority: "high" },
              ],
            }),
          },
          {
            type: "tool_result",
            tool_use_id: "todo-1",
            output_preview: "Todos have been modified successfully",
            is_error: false,
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.map((p) => p.type)).toEqual(["plan"])
    expect(adapted.content.some((p) => p.type === "tool-result")).toBe(false)
    const plan = adapted.content[0]
    if (plan.type !== "plan") throw new Error("expected a plan part")
    expect(plan.isStreaming).toBe(false)
    expect(plan.entries).toEqual([
      { content: "X", status: "pending", priority: "medium" },
      { content: "Y", status: "completed", priority: "high" },
    ])
  })

  it("does NOT convert a TodoWrite tool_use while streaming (live plan source is the synthetic block)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "live-todo",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: JSON.stringify({
              todos: [{ content: "X", status: "pending", priority: "medium" }],
            }),
          },
        ],
      },
      msgText,
      true
    )

    expect(adapted.content.every((p) => p.type !== "plan")).toBe(true)
  })

  it("falls back to a normal tool card when a plan-like tool has unparsable input", () => {
    const adapted = adaptMessageTurn(
      {
        id: "hist-bad",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        blocks: [
          {
            type: "tool_use",
            tool_use_id: "todo-1",
            tool_name: "TodoWrite",
            input_preview: "not json",
          },
        ],
      },
      msgText,
      false
    )

    expect(adapted.content.every((p) => p.type !== "plan")).toBe(true)
  })
})

describe("extractUserResourcesFromText — codeg references stay inline", () => {
  it("keeps a codeg://agent link inline (the @-prefixed label no longer lifts it to a chip)", () => {
    const input = "ask [@Codex](codeg://agent/codex) to review"
    const { text, resources } = extractUserResourcesFromText(input)
    expect(resources).toEqual([])
    expect(text).toBe(input)
  })

  it("keeps codeg://session and codeg://commit links inline", () => {
    const session = extractUserResourcesFromText(
      "see [#42](codeg://session/claude_code_abc)"
    )
    expect(session.resources).toEqual([])
    expect(session.text).toBe("see [#42](codeg://session/claude_code_abc)")

    const commit = extractUserResourcesFromText(
      "from [a1b2c3d](codeg://commit/%2Frepo@a1b2c3ddeadbeef)"
    )
    expect(commit.resources).toEqual([])
    expect(commit.text).toBe(
      "from [a1b2c3d](codeg://commit/%2Frepo@a1b2c3ddeadbeef)"
    )
  })

  it("keeps a codeg://session link inline even when its label starts with @ (a session titled '@…')", () => {
    const input = "ping [@周报](codeg://session/codex_99)"
    const { text, resources } = extractUserResourcesFromText(input)
    expect(resources).toEqual([])
    expect(text).toBe(input)
  })

  it("still lifts file:// links to the resource list (files unchanged this round)", () => {
    const { text, resources } = extractUserResourcesFromText(
      "look at [foo.ts](file:///x/foo.ts) here"
    )
    expect(resources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    expect(text).toBe("look at here")
  })

  it("still lifts blocked @-mentions to the resource list", () => {
    const { resources } = extractUserResourcesFromText(
      "@secret.txt [blocked: outside workspace]"
    )
    expect(resources).toEqual([
      { name: "secret.txt", uri: "secret.txt", mime_type: null },
    ])
  })

  it("splits a mixed message: file → chip, session → inline", () => {
    const { text, resources } = extractUserResourcesFromText(
      "compare [foo.ts](file:///x/foo.ts) with [#42](codeg://session/codex_abc)"
    )
    expect(resources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    expect(text).toContain("[#42](codeg://session/codex_abc)")
    expect(text).not.toContain("file://")
  })
})

describe("adaptMessageTurn — user reference resources", () => {
  const msgText = {
    attachedResources: "Attached resources",
    toolCallFailed: "Tool failed",
  }

  it("keeps an agent reference inline in the user turn (no chip row)", () => {
    const adapted = adaptMessageTurn(
      {
        id: "u1",
        role: "user",
        timestamp: "2026-06-11T00:00:00.000Z",
        blocks: [
          { type: "text", text: "ask [@Codex](codeg://agent/codex) to review" },
        ],
      },
      msgText
    )

    expect(adapted.userResources).toBeUndefined()
    expect(adapted.content).toHaveLength(1)
    const part = adapted.content[0]
    if (part.type !== "text") throw new Error("expected a text part")
    expect(part.text).toContain("[@Codex](codeg://agent/codex)")
  })

  it("routes a file to the chip row while keeping a session reference inline", () => {
    // Mirrors the backend fold: prose+session in one text block, the file
    // resource_link folded to a trailing `[name](uri)` text block.
    const adapted = adaptMessageTurn(
      {
        id: "u2",
        role: "user",
        timestamp: "2026-06-11T00:00:00.000Z",
        blocks: [
          {
            type: "text",
            text: "compare these [#42](codeg://session/codex_abc)",
          },
          { type: "text", text: "[foo.ts](file:///x/foo.ts)" },
        ],
      },
      msgText
    )

    expect(adapted.userResources).toEqual([
      { name: "foo.ts", uri: "file:///x/foo.ts", mime_type: null },
    ])
    const textParts = adapted.content.filter((p) => p.type === "text")
    expect(textParts).toHaveLength(1)
    const part = textParts[0]
    if (part.type !== "text") throw new Error("expected a text part")
    expect(part.text).toContain("[#42](codeg://session/codex_abc)")
    expect(part.text).not.toContain("file://")
  })
})
