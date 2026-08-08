import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { parseTaskEvent } from './events'

const event = {
  schemaVersion: 1,
  fileId: 'file-1',
  operation: 'transcription',
  runId: 'run-1',
  generation: 1,
  sequence: 2,
  kind: 'progress',
  payload: { current: 1, total: 10 },
}

describe('parseTaskEvent', () => {
  it('接受对象事件信封', () => {
    expect(parseTaskEvent(event)).toEqual(event)
  })

  it('兼容后端 JSON 字符串事件信封', () => {
    expect(parseTaskEvent(JSON.stringify(event))).toEqual(event)
  })

  it('拒绝无效事件信封', () => {
    expect(parseTaskEvent('{bad')).toBeNull()
    expect(parseTaskEvent({ kind: 'progress' })).toBeNull()
  })
})
