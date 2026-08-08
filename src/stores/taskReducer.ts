// 前端任务 reducer（P0-01）
//
// 把"事件 → 状态"转换提取成纯函数，为单元测试形成可测边界。
// 设计约定：
// - reducer 输入为当前操作状态 + 一个 TaskEvent，输出新状态或 `ignored`。
// - 只接受当前 `runId/generation` 且 `sequence > lastSequence` 的事件。
// - 非法转换返回 `ignored` 并记录一次 debug 日志，且不直接修改 Pinia 状态
//   （状态由调用方按 reducer 返回的候选状态替换）。
// - 转写与规整拥有独立状态，polish 失败只改规整状态，不污染转写状态。

export type TaskOperation = 'transcription' | 'polish'

export type OperationPhase =
  | 'idle'
  | 'starting'
  | 'processing'
  | 'running'
  | 'pauseRequested'
  | 'paused'
  | 'cancelRequested'
  | 'completed'
  | 'cancelled'
  | 'failed'

/** 操作通用状态（清单 1.4 OperationState） */
export interface OperationState {
  operation: TaskOperation
  phase: OperationPhase
  runId: string | null
  generation: number | null
  lastSequence: number
  error: string | null
  text: string
}

/** 转写状态（清单 1.4 TranscriptionState） */
export interface TranscriptionState extends OperationState {
  operation: 'transcription'
  currentSlice: number
  totalSlices: number
  checkpoint: { lastCompleted: number; totalSlices: number } | null
  stage: string
  wordCount: number
}

/** 规整状态（清单 1.4 PolishState） */
export interface PolishState extends OperationState {
  operation: 'polish'
  polishStage: 'idle' | 'sending' | 'receiving' | 'failed'
  wordCount: number
}

/** 统一后端事件（序列化后 camelCase，与清单 1.1 对齐） */
export interface TaskEvent {
  schemaVersion: number
  fileId: string
  operation: TaskOperation
  runId: string
  generation: number
  sequence: number
  kind: TaskEventKind
  payload: Record<string, unknown>
}

export type CommandOutcomeKind =
  | 'completed'
  | 'paused'
  | 'cancelled'
  | 'failed'

/** 命令结果（清单 1.2 TaskRunOutcome），作为最终校准输入 */
export interface CommandOutcome {
  status: CommandOutcomeKind
  runId: string
  generation: number
  text?: string
  checkpoint?: { lastCompleted: number; totalSlices: number }
  error?: string
}

export type TaskEventKind =
  | 'started'
  | 'stage'
  | 'progress'
  | 'chunk'
  | 'paused'
  | 'completed'
  | 'cancelled'
  | 'failed'

export type ReducerResult =
  | { outcome: 'applied'; state: OperationState }
  | { outcome: 'ignored'; reason: string }

/** 初始转写状态 */
export function initialTranscriptionState(): TranscriptionState {
  return {
    operation: 'transcription',
    phase: 'idle',
    runId: null,
    generation: null,
    lastSequence: 0,
    error: null,
    text: '',
    currentSlice: 0,
    totalSlices: 0,
    checkpoint: null,
    stage: '',
    wordCount: 0,
  }
}

/** 初始规整状态 */
export function initialPolishState(): PolishState {
  return {
    operation: 'polish',
    phase: 'idle',
    runId: null,
    generation: null,
    lastSequence: 0,
    error: null,
    text: '',
    polishStage: 'idle',
    wordCount: 0,
  }
}

const TERMINAL_PHASES: ReadonlySet<OperationPhase> = new Set([
  'completed',
  'cancelled',
  'failed',
])

/** 更新已存在状态的通用字段（保留 operation 特有字段） */
function baseApply<T extends OperationState>(
  state: T,
  event: TaskEvent,
): T {
  return {
    ...state,
    runId: event.runId,
    generation: event.generation,
    lastSequence: event.sequence,
  }
}

function isFreshEvent(state: OperationState, event: TaskEvent): boolean {
  // 尚无绑定代际：接受首个事件
  if (state.generation === null) return true
  // 代际不匹配：旧事件，忽略
  if (state.generation !== event.generation) return false
  // 代际匹配但 sequence 不递增：重复/乱序，忽略（幂等）
  if (event.sequence <= state.lastSequence) return false
  return true
}

/** 事件操作类型必须与状态类型一致（转写事件只进入转写状态，反之亦然）。 */
function operationMatches(state: OperationState, event: TaskEvent): boolean {
  return state.operation === event.operation
}

/**
 * 核心 reducer：把单个任务事件应用到操作状态。
 * 返回 `applied`（携带候选新状态）或 `ignored`（含原因）。
 */
export function taskReducer(
  state: OperationState,
  event: TaskEvent,
): ReducerResult {
  // 事件必须属于当前 bind 的 runId
  if (state.runId !== null && state.runId !== event.runId) {
    return { outcome: 'ignored', reason: `runId 不匹配: ${event.runId}` }
  }
  if (!operationMatches(state, event)) {
    return {
      outcome: 'ignored',
      reason: `操作类型不匹配: ${event.operation}`,
    }
  }
  if (!isFreshEvent(state, event)) {
    return {
      outcome: 'ignored',
      reason: `sequence 已过期或重复: ${event.sequence} <= ${state.lastSequence}`,
    }
  }

  // 终态之后的事件一律忽略（completed 后 progress、cancelled 后 completed 等）
  if (TERMINAL_PHASES.has(state.phase)) {
    return { outcome: 'ignored', reason: `终态 ${state.phase} 后忽略事件` }
  }

  switch (event.kind) {
    case 'started':
      return {
        outcome: 'applied',
        state: baseApply({ ...state, phase: 'starting', error: null }, event),
      }
    case 'stage':
      if (state.operation === 'transcription') {
        const s = state as TranscriptionState
        return {
          outcome: 'applied',
          state: baseApply(
            { ...s, phase: 'processing', stage: String(event.payload.stage ?? '') },
            event,
          ),
        }
      }
      return { outcome: 'ignored', reason: 'polish 无 stage 事件' }
    case 'progress': {
      if (state.operation !== 'transcription') {
        return { outcome: 'ignored', reason: 'polish 无 progress 事件' }
      }
      const s = state as TranscriptionState
      const current = Number(event.payload.current ?? 0)
      const total = Number(event.payload.total ?? 0)
      return {
        outcome: 'applied',
        state: baseApply(
          {
            ...s,
            phase: 'running',
            currentSlice: current,
            totalSlices: total > 0 ? total : s.totalSlices,
            stage: 'transcribing',
          },
          event,
        ),
      }
    }
    case 'chunk': {
      const text = String(event.payload.text ?? '')
      return {
        outcome: 'applied',
        state: baseApply({ ...state, phase: 'running', text: state.text + text }, event),
      }
    }
    case 'paused': {
      if (state.operation !== 'transcription') {
        return { outcome: 'ignored', reason: 'polish 无 paused 事件' }
      }
      const s = state as TranscriptionState
      const lastCompleted = Number(event.payload.lastCompleted ?? 0)
      const total = Number(event.payload.totalSlices ?? s.totalSlices)
      return {
        outcome: 'applied',
        state: baseApply(
          {
            ...s,
            phase: 'paused',
            checkpoint: { lastCompleted, totalSlices: total },
            currentSlice: lastCompleted,
            totalSlices: total,
          },
          event,
        ),
      }
    }
    case 'completed': {
      const finalText = typeof event.payload.text === 'string' ? event.payload.text : state.text
      return {
        outcome: 'applied',
        state: baseApply({ ...state, phase: 'completed', error: null, text: finalText }, event),
      }
    }
    case 'cancelled':
      return {
        outcome: 'applied',
        state: baseApply({ ...state, phase: 'cancelled' }, event),
      }
    case 'failed':
      return {
        outcome: 'applied',
        state: baseApply(
          {
            ...state,
            phase: 'failed',
            error: String(event.payload.error ?? '未知错误'),
          },
          event,
        ),
      }
  }
}

/**
 * 命令结果校准（P2-06）：终态结果以后端完整文本替换流式 buffer。
 * 仅当 runId/generation 匹配时应用。
 */
export function applyCommandOutcome(
  state: OperationState,
  outcome: CommandOutcome,
): ReducerResult {
  if (state.generation !== null && state.generation !== outcome.generation) {
    return { outcome: 'ignored', reason: 'outcome 代际不匹配' }
  }
  if (state.runId !== null && state.runId !== outcome.runId) {
    return { outcome: 'ignored', reason: 'outcome runId 不匹配' }
  }
  switch (outcome.status) {
    case 'completed':
      return {
        outcome: 'applied',
        state: {
          ...state,
          phase: 'completed',
          error: null,
          text: outcome.text ?? state.text,
        },
      }
    case 'paused': {
      if (state.operation !== 'transcription') {
        return { outcome: 'ignored', reason: 'polish 无 paused outcome' }
      }
      const s = state as TranscriptionState
      return {
        outcome: 'applied' as const,
        state: {
          ...s,
          phase: 'paused',
          checkpoint: outcome.checkpoint ?? s.checkpoint,
          currentSlice: outcome.checkpoint?.lastCompleted ?? s.currentSlice,
          totalSlices: outcome.checkpoint?.totalSlices ?? s.totalSlices,
          text: outcome.text ?? s.text,
        } as TranscriptionState,
      }
    }
    case 'cancelled':
      return { outcome: 'applied', state: { ...state, phase: 'cancelled' } }
    case 'failed':
      return {
        outcome: 'applied',
        state: {
          ...state,
          phase: 'failed',
          error: outcome.error ?? '未知错误',
        },
      }
  }
}
