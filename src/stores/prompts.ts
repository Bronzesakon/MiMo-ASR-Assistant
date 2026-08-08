// 提示词 store（清单 F-06.4.1）：只保存正式 PromptDocument、加载/写操作状态
// 与调用后端命令的方法；不与 API Key、文件流式状态混合。
// 草稿状态不进此 store（局部在 PromptManager），避免未保存内容被其它页面误用。
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { PromptDocument } from '../types'

export const PROMPT_LIMIT = 8
export const TITLE_MAX_CHARS = 24
export const CONTENT_MAX_CHARS = 4000

export const usePromptsStore = defineStore('prompts', () => {
  const doc = ref<PromptDocument | null>(null)
  const loading = ref(false)
  const writing = ref(false)
  const loadFailed = ref(false)

  // 同一会话只执行一次正常加载，失败可显式重试（F-06.4.4）
  let loadedOnce = false
  // 内置默认正文缓存（「恢复默认」专用，正文事实来源 = 后端常量）
  let builtinDefault: string | null = null

  const ready = computed(() => doc.value !== null)
  const prompts = computed(() => doc.value?.prompts ?? [])
  const selectedPromptId = computed(() => doc.value?.selected_prompt_id ?? '')
  const atLimit = computed(() => (doc.value?.prompts.length ?? 0) >= PROMPT_LIMIT)

  async function ensureLoaded(force = false): Promise<boolean> {
    if (doc.value && !force) return true
    if (loadedOnce && !force) return doc.value !== null
    if (loading.value) return doc.value !== null
    loading.value = true
    try {
      doc.value = await invoke<PromptDocument>('load_prompts')
      loadedOnce = true
      loadFailed.value = false
      return true
    } catch (e) {
      console.error('[提示词] 加载失败:', e)
      loadFailed.value = true
      return false
    } finally {
      loading.value = false
    }
  }

  async function createPrompt(title: string, content: string): Promise<PromptDocument> {
    if (writing.value) throw new Error('正在写入，请稍候')
    writing.value = true
    try {
      doc.value = await invoke<PromptDocument>('create_prompt', { title, content })
      return doc.value
    } finally {
      writing.value = false
    }
  }

  async function updatePrompt(id: string, title: string | null, content: string): Promise<PromptDocument> {
    if (writing.value) throw new Error('正在写入，请稍候')
    writing.value = true
    try {
      doc.value = await invoke<PromptDocument>('update_prompt', { id, title, content })
      return doc.value
    } finally {
      writing.value = false
    }
  }

  async function deletePrompt(id: string): Promise<PromptDocument> {
    if (writing.value) throw new Error('正在写入，请稍候')
    writing.value = true
    try {
      doc.value = await invoke<PromptDocument>('delete_prompt', { id })
      return doc.value
    } finally {
      writing.value = false
    }
  }

  async function reorderPrompts(orderedIds: string[]): Promise<PromptDocument> {
    if (writing.value) throw new Error('正在写入，请稍候')
    writing.value = true
    try {
      doc.value = await invoke<PromptDocument>('reorder_prompts', { orderedIds })
      return doc.value
    } finally {
      writing.value = false
    }
  }

  /** 选择最近项：先原子持久化成功才更新文档；失败抛出，调用方不得开始规整 */
  async function selectPrompt(id: string): Promise<PromptDocument> {
    doc.value = await invoke<PromptDocument>('select_prompt', { id })
    return doc.value
  }

  /** 「恢复默认」只改草稿；正文事实来源 = 后端 builtin_default_content */
  async function builtinDefaultContent(): Promise<string> {
    if (builtinDefault !== null) return builtinDefault
    builtinDefault = await invoke<string>('builtin_default_content')
    return builtinDefault
  }

  function openPromptDir(): void {
    invoke('open_prompt_dir').catch((e) => console.error('[提示词] 打开目录失败:', e))
  }

  return {
    doc, loading, writing, loadFailed,
    ready, prompts, selectedPromptId, atLimit,
    ensureLoaded, createPrompt, updatePrompt, deletePrompt,
    reorderPrompts, selectPrompt, builtinDefaultContent, openPromptDir,
  }
})
