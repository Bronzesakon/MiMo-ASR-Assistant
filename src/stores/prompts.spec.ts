// C-03 提示词 store 写失败语义：任何写命令失败都必须保持正式文档原样
// （保存失败后草稿/正式状态保留；reorder 失败后顺序回滚；select 失败后选中项不变，
//  调用方据此不得开始规整）。
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import type { PromptDocument } from '../types'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

import { invoke } from '@tauri-apps/api/core'
import { usePromptsStore } from './prompts'

const mockInvoke = invoke as unknown as ReturnType<typeof vi.fn>

function doc(): PromptDocument {
  return {
    schema_version: 1,
    selected_prompt_id: 'builtin-default',
    prompts: [
      { id: 'builtin-default', title: '默认', content: '默认正文', is_default: true },
      { id: 'a', title: '条目A', content: '正文A', is_default: false },
      { id: 'b', title: '条目B', content: '正文B', is_default: false },
    ],
  }
}

describe('提示词 store 写失败语义（C-03）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockInvoke.mockReset()
  })

  it('create 失败后 doc 保持原样并抛错', async () => {
    const store = usePromptsStore()
    const before = doc()
    store.doc = before
    mockInvoke.mockRejectedValueOnce(new Error('写盘失败'))
    await expect(store.createPrompt('t', 'c')).rejects.toThrow('写盘失败')
    expect(store.doc).toStrictEqual(before)
  })

  it('update 失败后 doc 保持原样并抛错', async () => {
    const store = usePromptsStore()
    const before = doc()
    store.doc = before
    mockInvoke.mockRejectedValueOnce(new Error('写盘失败'))
    await expect(store.updatePrompt('a', '新标题', '新正文')).rejects.toThrow('写盘失败')
    expect(store.doc).toStrictEqual(before)
    expect(store.doc!.prompts[1].title).toBe('条目A')
  })

  it('reorder 失败后 doc 保持原样（顺序回滚）', async () => {
    const store = usePromptsStore()
    const before = doc()
    store.doc = before
    mockInvoke.mockRejectedValueOnce(new Error('排序失败'))
    await expect(
      store.reorderPrompts(['builtin-default', 'b', 'a'])
    ).rejects.toThrow('排序失败')
    // 顺序未变：仍为 a 在 b 前
    expect(store.doc!.prompts.map(p => p.id)).toEqual(['builtin-default', 'a', 'b'])
  })

  it('select 失败后 selectedPromptId 与 doc 不变', async () => {
    const store = usePromptsStore()
    const before = doc()
    store.doc = before
    mockInvoke.mockRejectedValueOnce(new Error('选择失败'))
    await expect(store.selectPrompt('b')).rejects.toThrow('选择失败')
    expect(store.doc).toStrictEqual(before)
    expect(store.selectedPromptId).toBe('builtin-default')
  })

  it('select 成功后 doc 更新为后端返回', async () => {
    const store = usePromptsStore()
    store.doc = doc()
    const updated = doc()
    updated.selected_prompt_id = 'b'
    mockInvoke.mockResolvedValueOnce(updated)
    const result = await store.selectPrompt('b')
    expect(result).toStrictEqual(updated)
    expect(store.selectedPromptId).toBe('b')
  })
})