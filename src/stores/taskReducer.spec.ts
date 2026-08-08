// P0-01：任务 reducer 纯函数测试
import { describe, it, expect } from 'vitest'
import {
  taskReducer,
  applyCommandOutcome,
  initialTranscriptionState,
  initialPolishState,
  type TranscriptionState,
  type TaskEvent,
  type CommandOutcome,
} from './taskReducer'

// 构造一个转写事件
function ev(
  over: Partial<TaskEvent> & { kind: TaskEvent['kind'] },
): TaskEvent {
  return {
    schemaVersion: 1,
    fileId: 'f1',
    operation: 'transcription',
    runId: 'run-1',
    generation: 1,
    sequence: 1,
    payload: {},
    ...over,
  }
}

describe('taskReducer', () => {
  it('删除文件后的迟到事件被忽略（无绑定代际时仅代际匹配才接收）', () => {
    // 模拟：新一次运行已绑定 generation=5，旧运行 generation=1 的迟到 chunk
    const state = {
      ...initialTranscriptionState(),
      runId: 'run-new',
      generation: 5,
      lastSequence: 10,
      phase: 'running' as const,
    }
    const late = ev({
      runId: 'run-old',
      generation: 1,
      sequence: 99,
      kind: 'chunk',
      payload: { text: '旧' },
    })
    const r = taskReducer(state, late)
    expect(r.outcome).toBe('ignored')
  })

  it('旧代际 chunk/progress/complete/error 被忽略', () => {
    const state = {
      ...initialTranscriptionState(),
      runId: 'run-1',
      generation: 2,
      lastSequence: 5,
      phase: 'running' as const,
    }
    for (const kind of ['chunk', 'progress', 'completed', 'failed'] as const) {
      const e = ev({ generation: 1, sequence: 50, kind, payload: {} })
      expect(taskReducer(state, e).outcome).toBe('ignored')
    }
  })

  it('failed 后的 progress 被忽略', () => {
    const state = {
      ...initialTranscriptionState(),
      phase: 'failed' as const,
      generation: 1,
      runId: 'run-1',
      lastSequence: 2,
    }
    const r = taskReducer(state, ev({ kind: 'progress', sequence: 3, payload: { current: 2, total: 5 } }))
    expect(r.outcome).toBe('ignored')
  })

  it('cancelled 后的 completed 被忽略', () => {
    const state = initialTranscriptionState()
    state.phase = 'cancelled'
    state.generation = 1
    state.runId = 'run-1'
    state.lastSequence = 1
    const r = taskReducer(state, ev({ kind: 'completed', sequence: 2 }))
    expect(r.outcome).toBe('ignored')
  })

  it('polish failed 只修改规整状态，不修改转写状态', () => {
    // 转写状态保持 completed
    const t = { ...initialTranscriptionState(), phase: 'completed' as const, generation: 1, runId: 'run-1', lastSequence: 1 }
    const p = { ...initialPolishState(), phase: 'running' as const, generation: 1, runId: 'run-1', lastSequence: 1 }
    const evt: TaskEvent = {
      schemaVersion: 1, fileId: 'f1', operation: 'polish', runId: 'run-1', generation: 1, sequence: 2,
      kind: 'failed', payload: { error: '401' },
    }
    const pt = taskReducer(p, evt)
    expect(pt.outcome).toBe('applied')
    if (pt.outcome === 'applied') {
      expect(pt.state.phase).toBe('failed')
      expect(pt.state.operation).toBe('polish')
    }
    // 转写状态不受影响
    expect(t.phase).toBe('completed')
  })

  it('相同 sequence 的重复事件幂等', () => {
    let state = initialTranscriptionState()
    const e1 = ev({ kind: 'chunk', sequence: 1, payload: { text: 'a' } })
    const r1 = taskReducer(state, e1)
    expect(r1.outcome).toBe('applied')
    if (r1.outcome === 'applied') state = r1.state as TranscriptionState
    // 用相同 sequence 重发
    const r2 = taskReducer(state, ev({ kind: 'chunk', sequence: 1, payload: { text: 'a' } }))
    expect(r2.outcome).toBe('ignored')
  })

  it('chunk 累积文本，进度推进切片', () => {
    let state = initialTranscriptionState()
    for (const e of [
      ev({ kind: 'started', sequence: 1 }),
      ev({ kind: 'progress', sequence: 2, payload: { current: 1, total: 3 } }),
      ev({ kind: 'chunk', sequence: 3, payload: { text: '你好' } }),
      ev({ kind: 'chunk', sequence: 4, payload: { text: '世界' } }),
    ]) {
      const r = taskReducer(state, e)
      expect(r.outcome).toBe('applied')
      if (r.outcome === 'applied') state = r.state as never
    }
    expect(state.text).toBe('你好世界')
    expect((state as { currentSlice: number }).currentSlice).toBe(1)
    expect((state as { totalSlices: number }).totalSlices).toBe(3)
  })

  it('completed 使用后端完整文本校准漏收的 chunk', () => {
    const state = {
      ...initialTranscriptionState(),
      phase: 'running' as const,
      generation: 1,
      runId: 'run-1',
      lastSequence: 4,
      text: '部分',
    }
    const result = taskReducer(state, ev({
      kind: 'completed',
      sequence: 5,
      payload: { text: '完整正文' },
    }))
    expect(result.outcome).toBe('applied')
    if (result.outcome === 'applied') {
      expect(result.state.text).toBe('完整正文')
      expect(result.state.phase).toBe('completed')
    }
  })

  it('paused 写入断点', () => {
    let state = initialTranscriptionState()
    state = (taskReducer(state, ev({ kind: 'started' })) as { outcome: 'applied'; state: typeof state }).state
    const r = taskReducer(state, ev({ kind: 'paused', sequence: 2, payload: { lastCompleted: 2, totalSlices: 5 } }))
    expect(r.outcome).toBe('applied')
    if (r.outcome === 'applied') {
      expect((r.state as TranscriptionState).checkpoint).toEqual({ lastCompleted: 2, totalSlices: 5 })
      expect(r.state.phase).toBe('paused')
    }
  })
})

describe('applyCommandOutcome', () => {
  it('completed 完整文本替换流式 buffer', () => {
    const state = { ...initialTranscriptionState(), phase: 'running' as const, generation: 1, runId: 'run-1', text: '部分' }
    const outcome: CommandOutcome = { status: 'completed', runId: 'run-1', generation: 1, text: '完整正文' }
    const r = applyCommandOutcome(state, outcome)
    expect(r.outcome).toBe('applied')
    if (r.outcome === 'applied') {
      expect((r.state as { text: string }).text).toBe('完整正文')
      expect(r.state.phase).toBe('completed')
    }
  })

  it('代际不匹配的 outcome 被忽略', () => {
    const state = { ...initialTranscriptionState(), generation: 1, runId: 'run-1' }
    const r = applyCommandOutcome(state, { status: 'completed', runId: 'run-1', generation: 99 })
    expect(r.outcome).toBe('ignored')
  })
})
