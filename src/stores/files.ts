// 文件 store（清单 F-04 + 新任务事件协议）：文件队列、处理状态、渲染节流、转写动作编排。
// 清单 F-02：流式渲染消除 O(n²)——字数统计增量化，完成事件时用全文 countWords 校准；
// 渲染策略采用原文方案 a：流式阶段（transcribing/polishing）恒走 escapeHtml 纯文本路径，
// 完成事件后再按最终内容判定是否切换 markdown 渲染。
// 新协议：每个文件维护 {transcription, polish} 两套 OperationState（taskStates Map），
// 事件经 taskReducer 纯函数归约，applied 才同步回 FileInfo。
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { FileInfo, TaskOperation, AddFileResult } from '../types'
import {
  taskReducer,
  initialTranscriptionState,
  initialPolishState,
  type TaskEvent,
  type OperationState,
  type TranscriptionState,
  type PolishState,
} from './taskReducer'
import { useSettingsStore } from './settings'
import { useUiStore } from './ui'

export interface FileProcessState {
  transcribeStartTime: number
  outputCharCount: number
  totalSlices: number
  currentSlice: number
  transcriptionChunks: number
  polishChunkCount: number
  polishFirstChunk: boolean
  /** 清单 F-02：流式期间增量累计的字数（完成时用全文 countWords 校准） */
  transcriptionWords: number
  polishWords: number
}

export type RenderTarget = 'transcription' | 'polish'

export const useFilesStore = defineStore('files', () => {
  const settings = useSettingsStore()
  const ui = useUiStore()

  const files = ref<FileInfo[]>([])
  const selectedFileId = ref<string | null>(null)

  const fileStates = new Map<string, FileProcessState>()
  // 新协议：每个文件两套任务状态（转写/规整），taskReducer 的唯一归约驻地
  const taskStates = new Map<string, { transcription: TranscriptionState; polish: PolishState }>()

  const globalTokenCount = ref(0)
  const speedTick = ref(0)

  // 渲染节流：流式 chunk 到达时不立即触发渲染，等待 150ms 静默后再重绘
  const transcriptionRenderTick = ref(0)
  const polishRenderTick = ref(0)
  const DEBOUNCE_MS = 150
  const renderTimers = new Map<string, ReturnType<typeof setTimeout>>()

  function scheduleRender(fileId: string, type: RenderTarget) {
    const key = `${fileId}_${type}`
    const existing = renderTimers.get(key)
    if (existing) clearTimeout(existing)
    renderTimers.set(key, setTimeout(() => {
      renderTimers.delete(key)
      if (type === 'transcription') transcriptionRenderTick.value++
      else polishRenderTick.value++
    }, DEBOUNCE_MS))
  }

  function flushRender(fileId: string) {
    for (const type of ['transcription', 'polish'] as const) {
      const key = `${fileId}_${type}`
      const timer = renderTimers.get(key)
      if (timer) { clearTimeout(timer); renderTimers.delete(key) }
    }
    transcriptionRenderTick.value++
    polishRenderTick.value++
  }

  const selectedFile = computed(() =>
    files.value.find(f => f.id === selectedFileId.value) || null
  )

  const completedCount = computed(() =>
    files.value.filter(f => f.status === 'completed').length
  )

  const hasActiveFiles = computed(() =>
    files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
  )

  const statusInfo = computed(() => {
    void speedTick.value // 每秒强制重算输出速度
    let totalOutputSpeed = 0

    for (const [fileId, state] of fileStates) {
      const file = files.value.find(f => f.id === fileId)
      if (!file || (file.status !== 'transcribing' && file.status !== 'processing')) continue

      if (state.transcribeStartTime > 0) {
        const elapsed = (Date.now() - state.transcribeStartTime) / 1000
        if (elapsed > 0) totalOutputSpeed += Math.round(state.outputCharCount / elapsed)
      }
    }

    return {
      modelName: settings.committedConfig.transcriptionModel,
      outputSpeed: totalOutputSpeed,
      tokenCount: globalTokenCount.value,
    }
  })

  // ============================================================
  // 文件增删改
  // ============================================================

  /**
   * 添加文件（P5）：返回加入结果，调用方据此决定是否选中/提示。
   * 区别名派发：仅 added 时调用方才选择新文件。
   */
  function addFile(file: FileInfo, maxFiles = 500): AddFileResult {
    if (files.value.some(f => f.path === file.path)) return 'duplicate'
    if (files.value.length >= maxFiles) return 'limitReached'
    files.value.push(file)
    console.log(`[文件] 导入: ${file.name}, 时长=${file.duration.toFixed(1)}s, id=${file.id}`)
    return 'added'
  }

  function removeFile(id: string) {
    const index = files.value.findIndex(f => f.id === id)
    if (index > -1) {
      files.value.splice(index, 1)
      fileStates.delete(id)
      taskStates.delete(id)
      flushRender(id)
      if (selectedFileId.value === id) selectedFileId.value = files.value[0]?.id || null
    }
  }

  function updateFile(id: string, updates: Partial<FileInfo>) {
    const file = files.value.find(f => f.id === id)
    if (file) Object.assign(file, updates)
  }

  function selectFile(id: string) {
    if (files.value.some(file => file.id === id)) selectedFileId.value = id
  }

  function resetStatus() {
    globalTokenCount.value = 0
    fileStates.clear()
    taskStates.clear()
  }

  function countWords(text: string): number {
    if (!text) return 0
    const chinese = (text.match(/[一-鿿]/g) || []).length
    const english = text.replace(/[一-鿿]/g, ' ').trim().split(/\s+/).filter(w => w.length > 0).length
    return chinese + english
  }

  /**
   * 清单 F-02：只统计增量 chunk 的字数，避免每个 chunk 对全文正则扫描（O(n²)）。
   * 中文按字符计数可精确增量；英文按空白分词在 chunk 边界会有误差（如 "hel"+"lo"），
   * 可接受，在 completed 时用全文 countWords 校准一次。
   */
  function countWordsDelta(chunk: string): number {
    if (!chunk) return 0
    return countWords(chunk)
  }

  function ensureFileState(fileId: string): FileProcessState {
    let state = fileStates.get(fileId)
    if (!state) {
      state = {
        transcribeStartTime: 0, outputCharCount: 0,
        totalSlices: 0, currentSlice: 0,
        transcriptionChunks: 0,
        polishChunkCount: 0, polishFirstChunk: false,
        transcriptionWords: 0, polishWords: 0,
      }
      fileStates.set(fileId, state)
    }
    return state
  }

  // ============================================================
  // 任务状态（新协议）：taskStates Map + reducer 归约 + FileInfo 同步
  // ============================================================

  function ensureTaskStates(fileId: string): { transcription: TranscriptionState; polish: PolishState } {
    let entry = taskStates.get(fileId)
    if (!entry) {
      entry = { transcription: initialTranscriptionState(), polish: initialPolishState() }
      taskStates.set(fileId, entry)
    }
    return entry
  }

  /** 启动一轮任务：初始化对应 OperationState（runId 由调用方生成），合理保留已有文本 */
  function startTask(fileId: string, operation: TaskOperation, runId: string) {
    const file = files.value.find(f => f.id === fileId)
    const states = ensureTaskStates(fileId)
    const baseText = operation === 'transcription'
      ? (file?.transcription || '')
      : (file?.polished || '')

    const procState = ensureFileState(fileId)
    if (operation === 'transcription') {
      states.transcription = { ...initialTranscriptionState(), runId, text: baseText }
      procState.transcribeStartTime = Date.now()
      procState.outputCharCount = 0
      procState.totalSlices = 0
      procState.currentSlice = 0
      procState.transcriptionWords = 0
    } else {
      states.polish = { ...initialPolishState(), runId, text: baseText }
      procState.polishChunkCount = 0
      procState.polishFirstChunk = false
      procState.polishWords = 0
    }
    if (file) { file.runId = runId; file.runGeneration = undefined }
  }

  /** invoke 返回 generation 后回填到对应任务状态（及 FileInfo.runGeneration） */
  function setTaskGeneration(fileId: string, operation: TaskOperation, generation: number) {
    const states = taskStates.get(fileId)
    if (!states) return
    if (operation === 'transcription') {
      states.transcription = { ...states.transcription, generation }
    } else {
      states.polish = { ...states.polish, generation }
    }
    const file = files.value.find(f => f.id === fileId)
    if (file) file.runGeneration = generation
  }

  /** 把 OperationState 归约结果同步回 FileInfo（状态/进度/文本/stage） */
  function syncOpState(file: FileInfo, op: TaskOperation, state: OperationState) {
    if (op === 'transcription') {
      const s = state as TranscriptionState
      file.transcription = s.text
      file.transcriptionWordCount = countWords(s.text)
      file.stage = s.stage
      switch (s.phase) {
        case 'idle': file.status = 'waiting'; break
        case 'starting':
        case 'processing': file.status = 'processing'; break
        case 'running':
          file.status = 'transcribing'
          file.progress = s.totalSlices > 0
            ? Math.min(100, Math.round(s.currentSlice / s.totalSlices * 100))
            : file.progress
          break
        case 'pauseRequested': file.status = 'pausing'; break
        case 'paused':
          file.status = 'paused'
          file.lastCompletedIndex = s.checkpoint?.lastCompleted ?? file.lastCompletedIndex
          file.resumeTotal = s.checkpoint?.totalSlices ?? file.resumeTotal
          break
        case 'cancelRequested': file.status = 'cancelling'; break
        case 'completed': file.status = 'completed'; file.progress = 100; file.error = undefined; break
        case 'cancelled': file.status = 'cancelled'; break
        case 'failed': file.status = 'failed'; file.error = s.error ?? file.error; break
      }
    } else {
      const s = state as PolishState
      file.polished = s.text
      file.polishedWordCount = countWords(s.text)
      switch (s.phase) {
        case 'running':
          file.status = 'polishing'
          file.polishStage = 'receiving'
          break
        case 'completed':
          if (file.status === 'polishing') file.status = 'completed'
          file.polishStage = 'idle'
          file.error = undefined
          break
        case 'cancelled':
          if (file.status === 'polishing') file.status = 'completed'
          file.polishStage = 'idle'
          break
        case 'failed':
          if (file.status === 'polishing') file.status = 'completed'
          file.polishStage = 'failed'
          file.error = s.error ?? file.error
          break
        default:
          break
      }
    }
  }

  /**
   * 应用一个 TaskEvent 到对应任务状态（事件层调用）。
   * 返回 true 表示 applied（已同步 FileInfo）；false 表示 ignored（代际/runId/终态不匹配）。
   */
  function reduceTaskEvent(event: TaskEvent): boolean {
    const file = files.value.find(f => f.id === event.fileId)
    if (!file) return false
    const states = ensureTaskStates(event.fileId)
    const op: TaskOperation = event.operation
    const current = states[op]
    const r = taskReducer(current, event)
    if (r.outcome === 'ignored') {
      console.warn('[任务事件] 已忽略', {
        fileId: event.fileId,
        operation: event.operation,
        kind: event.kind,
        sequence: event.sequence,
        reason: r.reason,
      })
      return false
    }

    states[op] = r.state as never
    syncOpState(file, op, r.state)

    // chunk 增量统计（驱动输出速度与字数显示器）
    if (event.kind === 'chunk') {
      const procState = ensureFileState(event.fileId)
      const text = String(event.payload.text ?? '')
      if (op === 'transcription') {
        procState.outputCharCount += text.length
        procState.transcriptionChunks++
        procState.transcriptionWords += countWordsDelta(text)
      } else {
        procState.polishChunkCount++
        procState.polishFirstChunk = true
        procState.polishWords += countWordsDelta(text)
      }
    }
    return true
  }

  // ============================================================
  // 转写/规整动作编排（取消 / 暂停 / 继续 / 清理）
  // ============================================================

  function runInfo(fileId: string, op: TaskOperation): { runId: string; generation: number } {
    const file = files.value.find(f => f.id === fileId)
    const states = taskStates.get(fileId)
    const s = states?.[op]
    return {
      runId: s?.runId || file?.runId || '',
      generation: s?.generation ?? file?.runGeneration ?? 0,
    }
  }

  /** 取消任务（P1-02：成功后由调用方 removeFile；失败则保留文件并提示） */
  function requestCancelFile(fileId: string, operation: TaskOperation = 'transcription'): Promise<void> {
    const file = files.value.find(f => f.id === fileId)
    if (!file) return Promise.resolve()
    const { runId, generation } = runInfo(fileId, operation)
    return invoke('cancel_task', { fileId, fileName: file.name, operation, runId, generation }).then(() => {})
  }

  function pauseTranscription(fileId: string) {
    const file = files.value.find(f => f.id === fileId)
    if (!file) return
    updateFile(fileId, { status: 'pausing' })
    ui.showToastMessage(`${file.name} 正在暂停转写`, 'info')
    const states = taskStates.get(fileId)
    if (states) states.transcription = { ...states.transcription, phase: 'pauseRequested' }
    const { runId, generation } = runInfo(fileId, 'transcription')
    invoke('pause_task', { fileId, fileName: file.name, runId, generation }).catch((error) => {
      const current = files.value.find(f => f.id === fileId)
      if (current?.status === 'pausing') updateFile(fileId, { status: 'transcribing' })
      if (states) states.transcription = { ...states.transcription, phase: 'running' }
      ui.showToastMessage(`${file.name} 暂停失败`, 'error')
      console.error('[暂停] 请求失败:', error)
    })
  }

  async function resumeTranscription(fileId: string) {
    const file = files.value.find(f => f.id === fileId)
    if (!file) return

    const runId = crypto.randomUUID()
    updateFile(fileId, { status: 'processing', progress: 0 })
    startTask(fileId, 'transcription', runId)
    ui.isTranscribing = true

    try {
      // 后端从注册表断点读取，不再传 tempDir/startIndex
      const generation = await invoke<number>('resume_transcription', {
        fileId,
        fileName: file.name,
        operation: 'transcription',
        runId,
        apiParams: {
          base_url: settings.asrBaseUrl,
          api_key: settings.asrKey,
          model: settings.committedConfig.transcriptionModel,
          language: file.language || 'auto',
        },
      })
      setTaskGeneration(fileId, 'transcription', generation)
    } catch (e) {
      console.error(`[转写] 继续转写失败: ${file.name}`, e)
      const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
      updateFile(fileId, { status: 'failed', error: errMsg })
      ui.showToastMessage(`${file.name} 继续转写失败`, 'error')
    }

    ui.isTranscribing = files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
  }

  /** 清理临时目录（新协议：按 fileId + runId + generation，不再传 tempDir） */
  function cleanupFileTemp(fileId: string): Promise<void> {
    const file = files.value.find(f => f.id === fileId)
    if (!file) return Promise.resolve()
    const { runId, generation } = runInfo(fileId, 'transcription')
    return invoke('cleanup_file_temp', { fileId, runId, generation }).then(() => {})
  }

  return {
    files, selectedFileId, selectedFile, completedCount, hasActiveFiles,
    fileStates, ensureFileState, resetStatus, countWords, countWordsDelta,
    globalTokenCount, speedTick, statusInfo,
    transcriptionRenderTick, polishRenderTick,
    scheduleRender, flushRender,
    addFile, removeFile, updateFile, selectFile,
    taskStates, ensureTaskStates, startTask, setTaskGeneration, reduceTaskEvent,
    requestCancelFile, pauseTranscription, resumeTranscription, cleanupFileTemp,
  }
})
