// 事件层抽象（清单 F-03 / 新任务事件协议）：
// - listenTyped<T>：listen<T> 直接消费 event.payload（后端现在 emit 对象，不再 emit 字符串），
//   unlisten 注册到模块级清理列表；注册失败向上抛错（P2-02 全有或全无）。
// - setupListeners / teardownListeners：由 App.vue 的 onMounted/onUnmounted 显式调用，
//   store 构造不再注册任何监听器（无副作用）。
import { listen } from '@tauri-apps/api/event'
import { useFilesStore } from '../stores/files'
import { useUiStore } from '../stores/ui'
import type {
  TaskEvent,
  TaskOperation,
  TokenUpdateEvent,
  PromptsRecoveryNoticeEvent,
} from '../types'

type FilesStore = ReturnType<typeof useFilesStore>
type UiStore = ReturnType<typeof useUiStore>

// 模块级清理列表：所有 listen 注册的 unlisten 在此汇总
const unlistenFns: Array<() => void> = []
let speedTimer: ReturnType<typeof setInterval> | null = null

/**
 * 监听后端事件并直接消费对象 payload（后端现在 emit 对象，不再经 string 序列化）。
 * 注册失败向上抛错，由 setupListeners 统一回滚（P2-02）。
 */
export async function listenTyped<T>(event: string, handler: (data: T) => void): Promise<void> {
  const unlisten = await listen<T>(event, (e) => handler(e.payload))
  unlistenFns.push(unlisten)
}

export function parseTaskEvent(payload: unknown): TaskEvent | null {
  let candidate = payload
  if (typeof payload === 'string') {
    try {
      candidate = JSON.parse(payload)
    } catch {
      console.error('[任务事件] JSON 解析失败')
      return null
    }
  }
  if (!candidate || typeof candidate !== 'object') return null
  const event = candidate as Partial<TaskEvent>
  if (!event.fileId || !event.runId || !event.operation || !event.kind) return null
  return event as TaskEvent
}

/**
 * 统一任务事件处理：解析 TaskEvent 信封 → 检查 operation/runId/generation 匹配
 * （由 taskReducer 完成）→ applied 则同步 FileInfo 并触发 UI 副作用；ignored 则丢弃。
 */
function handleTaskEvent(files: FilesStore, ui: UiStore, event: TaskEvent): void {
  const file = files.files.find(f => f.id === event.fileId)
  if (!file) return
  if (!files.reduceTaskEvent(event)) return

  const op: TaskOperation = event.operation
  switch (event.kind) {
    case 'chunk':
      files.scheduleRender(event.fileId, op)
      break
    case 'completed':
      files.flushRender(event.fileId)
      if (op === 'transcription') {
        if (event.fileId === files.selectedFileId) ui.showToastMessage(`${file.name} 转写完成`, 'success')
      } else {
        if (event.fileId === files.selectedFileId) ui.showToastMessage(`${file.name} 规整完成`, 'success')
      }
      break
    case 'paused':
      files.flushRender(event.fileId)
      if (event.fileId === files.selectedFileId) ui.showToastMessage(`${file.name} 已暂停转写`, 'info')
      break
    case 'cancelled':
      files.flushRender(event.fileId)
      if (event.fileId === files.selectedFileId) ui.showToastMessage(`${file.name} 已取消`, 'info')
      break
    case 'failed':
      files.flushRender(event.fileId)
      if (event.fileId === files.selectedFileId) {
        ui.showToastMessage(`${file.name} ${op === 'transcription' ? '转写' : '规整'}失败`, 'error')
      }
      break
    default:
      break
  }

  ui.isTranscribing = files.files.some(f =>
    f.status === 'transcribing' || f.status === 'processing'
  )
}

/**
 * 注册全部后端事件监听。仅在 App.vue onMounted 调用一次。
 * async：调用方 await 全部 listen 注册完成后，再触发 load_prompts 等 IPC，
 * 保证一次性恢复提示（prompts-recovery-notice）不丢失（H-01）。
 * 失败处理约定（P2-02 全有或全无）：任一关键监听注册失败则注销本轮已成功监听并抛错，
 * 由 App.vue 进入 initializationState='failed' 并显示重试按钮。
 * 转写/规整错误分离：transcription-error 只改转写状态，chat-error 只改规整状态。
 */
export async function setupListeners(files: FilesStore, ui: UiStore): Promise<void> {
  // 本轮已成功注册的 unlisten，用于回滚
  const roundUnlisten: Array<() => void> = []

  const reg = async <T>(event: string, handler: (data: T) => void): Promise<void> => {
    const unlisten = await listen<T>(event, (e) => handler(e.payload))
    roundUnlisten.push(unlisten)
    unlistenFns.push(unlisten)
  }

  const taskHandler = (payload: unknown) => {
    const event = parseTaskEvent(payload)
    if (event) handleTaskEvent(files, ui, event)
  }

  try {
    // 统一 TaskEvent 信封监听（kind 由信封决定，operation 由 reducer 校验）
    await reg<unknown>('process-status', taskHandler)
    await reg<unknown>('transcription-progress', taskHandler)
    await reg<unknown>('transcription-chunk', taskHandler)
    await reg<unknown>('transcription-paused', taskHandler)
    await reg<unknown>('transcription-complete', taskHandler)
    await reg<unknown>('transcription-cancelled', taskHandler)
    await reg<unknown>('transcription-error', taskHandler)
    await reg<unknown>('chat-chunk', taskHandler)
    await reg<unknown>('chat-complete', taskHandler)
    await reg<unknown>('chat-error', taskHandler)
    await reg<unknown>('chat-cancelled', taskHandler)

    // 旧格式（对象 payload，内容格式不变）：token-update、prompts-recovery-notice
    await reg<TokenUpdateEvent>('token-update', (data) => {
      const tokens = parseInt(data.text, 10)
      if (!isNaN(tokens)) files.globalTokenCount += tokens
    })

    await reg<PromptsRecoveryNoticeEvent>('prompts-recovery-notice', (data) => {
      const message = data && typeof data.message === 'string' ? data.message : ''
      if (message) ui.showToastMessage(message, 'info')
    })
  } catch (err) {
    // 全有或全无：回滚本轮已成功注册的监听
    for (const fn of roundUnlisten) fn()
    const set = new Set(roundUnlisten)
    for (let i = unlistenFns.length - 1; i >= 0; i--) {
      if (set.has(unlistenFns[i])) unlistenFns.splice(i, 1)
    }
    throw err
  }

  // 每秒刷新输出速度
  speedTimer = setInterval(() => {
    if (files.files.some(f => f.status === 'transcribing' || f.status === 'processing')) {
      files.speedTick++
    }
  }, 1000)
}

/** 注销全部监听与定时器（App.vue onUnmounted 调用） */
export function teardownListeners(): void {
  for (const fn of unlistenFns) fn()
  unlistenFns.length = 0
  if (speedTimer !== null) {
    clearInterval(speedTimer)
    speedTimer = null
  }
}
