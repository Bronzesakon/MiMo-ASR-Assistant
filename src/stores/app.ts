import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { FileInfo, ApiConfig } from '../types'

interface RustConfig {
  provider: string
  base_url: string
  api_key: string
  transcription_model: string
  polish_model: string
  theme: string
  polish_prompt: string
  provider_keys: Record<string, string>
}

interface FileProcessState {
  transcribeStartTime: number
  outputCharCount: number
  uploadSpeedSum: number
  uploadSpeedCount: number
  totalSlices: number
  currentSlice: number
  polishChunkCount: number
  polishFirstChunk: boolean
}

export const useAppStore = defineStore('app', () => {
  const files = ref<FileInfo[]>([])
  const selectedFileId = ref<string | null>(null)

  const polishEnabledFileId = ref<string | null>(null)
  const isSettingsOpen = ref(false)
  const isTranscribing = ref(false)
  const toastMessage = ref('')
  const toastType = ref<'info' | 'error' | 'success'>('info')
  const showToast = ref(false)

  const theme = ref<'system' | 'light' | 'dark'>('system')

  const apiConfig = ref<ApiConfig>({
    provider: 'mimo-api',
    baseUrl: 'https://api.xiaomimimo.com/v1',
    apiKey: '',
    transcriptionModel: 'mimo-v2.5-asr',
    polishModel: 'mimo-v2.5',
    polishPrompt: '给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，去掉口语化表达的语气词，要求只对每句话做修饰，每句话都要输出，不简略，根据原文的内容，智能切分长段落',
    providerKeys: {},
  })

  const fileStates = new Map<string, FileProcessState>()

  const statusInfo = computed(() => {
    let totalUploadSpeed = 0
    let totalOutputSpeed = 0

    for (const [fileId, state] of fileStates) {
      const file = files.value.find(f => f.id === fileId)
      if (!file || (file.status !== 'transcribing' && file.status !== 'processing')) continue

      if (state.uploadSpeedCount > 0) {
        totalUploadSpeed += state.uploadSpeedSum / state.uploadSpeedCount
      }
      if (state.transcribeStartTime > 0) {
        const elapsed = (Date.now() - state.transcribeStartTime) / 1000
        if (elapsed > 0) totalOutputSpeed += Math.round(state.outputCharCount / elapsed)
      }
    }

    return {
      modelName: apiConfig.value.transcriptionModel,
      uploadSpeed: totalUploadSpeed,
      outputSpeed: totalOutputSpeed,
      tokenCount: globalTokenCount.value,
    }
  })

  const globalTokenCount = ref(0)

  const selectedFile = computed(() =>
    files.value.find(f => f.id === selectedFileId.value) || null
  )

  const isPolishEnabled = computed(() =>
    polishEnabledFileId.value === selectedFileId.value
  )

  const completedCount = computed(() =>
    files.value.filter(f => f.status === 'completed').length
  )

  const hasActiveFiles = computed(() =>
    files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
  )

  function showToastMessage(message: string, type: 'info' | 'error' | 'success' = 'info') {
    toastMessage.value = message
    toastType.value = type
    showToast.value = true
    setTimeout(() => { showToast.value = false }, 3000)
  }

  async function loadConfig() {
    try {
      const config: RustConfig = await invoke('load_config')
      apiConfig.value = {
        provider: config.provider,
        baseUrl: config.base_url,
        apiKey: config.api_key,
        transcriptionModel: config.transcription_model,
        polishModel: config.polish_model,
        polishPrompt: config.polish_prompt || '给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，去掉口语化表达的语气词，要求只对每句话做修饰，每句话都要输出，不简略，根据原文的内容，智能切分长段落',
        providerKeys: config.provider_keys || {},
      }
      theme.value = (config.theme as 'system' | 'light' | 'dark') || 'system'
      applyTheme()
    } catch (e) {
      console.error('[配置] 加载失败:', e)
    }
  }

  async function saveConfig() {
    try {
      await invoke('save_config', {
        config: {
          provider: apiConfig.value.provider,
          base_url: apiConfig.value.baseUrl,
          api_key: apiConfig.value.apiKey,
          transcription_model: apiConfig.value.transcriptionModel,
          polish_model: apiConfig.value.polishModel,
          theme: theme.value,
          polish_prompt: apiConfig.value.polishPrompt,
          provider_keys: apiConfig.value.providerKeys,
        }
      })
    } catch (e) {
      console.error('[配置] 保存失败:', e)
    }
  }

  function applyTheme() {
    const root = document.documentElement
    root.removeAttribute('data-theme')
    if (theme.value === 'system') {
      const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches
      root.setAttribute('data-theme', prefersDark ? 'dark' : 'light')
    } else {
      root.setAttribute('data-theme', theme.value)
    }
  }

  function setTheme(t: 'system' | 'light' | 'dark') {
    theme.value = t
    applyTheme()
    saveConfig()
  }

  if (typeof window !== 'undefined') {
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
      if (theme.value === 'system') applyTheme()
    })
  }

  function addFile(file: FileInfo) {
    files.value.push(file)
    if (!selectedFileId.value) selectedFileId.value = file.id
    console.log(`[文件] 导入: ${file.name}, 时长=${file.duration.toFixed(1)}s, id=${file.id}`)
  }

  function removeFile(id: string) {
    const index = files.value.findIndex(f => f.id === id)
    if (index > -1) {
      files.value.splice(index, 1)
      fileStates.delete(id)
      if (selectedFileId.value === id) selectedFileId.value = files.value[0]?.id || null
      if (polishEnabledFileId.value === id) polishEnabledFileId.value = null
    }
  }

  function updateFile(id: string, updates: Partial<FileInfo>) {
    const file = files.value.find(f => f.id === id)
    if (file) Object.assign(file, updates)
  }

  function selectFile(id: string) { selectedFileId.value = id }
  function clearCompleted() { files.value = files.value.filter(f => f.status !== 'completed') }

  function setPolishEnabled(enabled: boolean) {
    if (enabled && selectedFileId.value) {
      polishEnabledFileId.value = selectedFileId.value
    } else {
      polishEnabledFileId.value = null
    }
  }

  function resetStatus() {
    globalTokenCount.value = 0
    fileStates.clear()
  }

  function countWords(text: string): number {
    if (!text) return 0
    const chinese = (text.match(/[一-鿿]/g) || []).length
    const english = text.replace(/[一-鿿]/g, ' ').trim().split(/\s+/).filter(w => w.length > 0).length
    return chinese + english
  }

  // ============================================================
  // 事件监听
  // ============================================================

  const unlistenFns: Array<() => void> = []

  function ensureFileState(fileId: string): FileProcessState {
    let state = fileStates.get(fileId)
    if (!state) {
      state = {
        transcribeStartTime: 0, outputCharCount: 0,
        uploadSpeedSum: 0, uploadSpeedCount: 0,
        totalSlices: 0, currentSlice: 0,
        polishChunkCount: 0, polishFirstChunk: false,
      }
      fileStates.set(fileId, state)
    }
    return state
  }

  function setupEventListeners() {
    // 转写文本 chunk — 按 file_id 路由
    listen<string>('transcription-chunk', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const text = data.text || ''
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          file.transcription += text
          file.transcriptionWordCount = countWords(file.transcription)
        }
        const state = ensureFileState(fileId)
        state.outputCharCount += text.length
      } catch (e) {
        console.error('[事件] transcription-chunk 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 转写进度 — 更新 currentSlice，驱动进度条
    listen<string>('transcription-progress', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const current = Number(data.current)
        const total = Number(data.total)
        const file = files.value.find(f => f.id === fileId)
        const state = ensureFileState(fileId)
        if (total > 0) state.totalSlices = total
        if (!isNaN(current) && current > state.currentSlice) {
          state.currentSlice = current
          console.log(`[转写] ${fileId} 分片进度: ${current}/${total}`)
        }
        // 进度 = 正在处理的分片序号 / 总分片数
        if (file && state.totalSlices > 0) {
          file.progress = Math.round((state.currentSlice / state.totalSlices) * 100)
        }
      } catch (e) {
        console.error('[事件] transcription-progress 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 转写完成
    listen<string>('transcription-complete', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          file.status = 'completed'
          file.progress = 100
          file.error = undefined
          file.transcriptionWordCount = countWords(file.transcription)
          console.log(`[转写] 完成: ${file.name}, 字数=${file.transcriptionWordCount}`)
        }
        fileStates.delete(fileId)
        isTranscribing.value = files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 转写完成`, 'success')
      } catch (e) {
        console.error('[事件] transcription-complete 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 转写失败
    listen<string>('transcription-error', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          file.status = 'failed'
          file.error = data.error || '未知错误'
          console.error(`[转写] 失败: ${file.name}, 错误=${data.error}`)
        }
        fileStates.delete(fileId)
        isTranscribing.value = files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 转写失败: ${data.error || ''}`, 'error')
      } catch (e) {
        console.error('[事件] transcription-error 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 规整 chunk — 按 file_id 路由，首次到达切换状态
    listen<string>('chat-chunk', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const text = data.text || ''
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          file.polished += text
          file.polishedWordCount = countWords(file.polished)
        }
        const state = ensureFileState(fileId)
        if (!state.polishFirstChunk) {
          state.polishFirstChunk = true
          console.log(`[规整] ${fileId} 开始接收流式数据`)
          // 更新 UI 状态为"接收中"
          if (file) updateFile(fileId, { polishStage: 'receiving' } as any)
        }
        state.polishChunkCount++
      } catch (e) {
        console.error('[事件] chat-chunk 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 规整完成
    listen<string>('chat-complete', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          if (file.status === 'polishing') file.status = 'completed'
          file.error = undefined
          file.polishedWordCount = countWords(file.polished)
          console.log(`[规整] 完成: ${file.name}, 字数=${file.polishedWordCount}`)
        }
        // 清理 polishStage
        if (file) (file as any).polishStage = 'idle'
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 规整完成`, 'success')
      } catch (e) {
        console.error('[事件] chat-complete 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 上传速度
    listen<string>('status-update', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const state = ensureFileState(fileId)
        if (data.upload_speed) {
          state.uploadSpeedSum += data.upload_speed
          state.uploadSpeedCount++
        }
      } catch {}
    }).then(fn => unlistenFns.push(fn))

    // Token 统计
    listen<string>('token-update', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const tokens = parseInt(data.text, 10)
        if (!isNaN(tokens)) globalTokenCount.value += tokens
      } catch {
        const tokens = parseInt(event.payload, 10)
        if (!isNaN(tokens)) globalTokenCount.value += tokens
      }
    }).then(fn => unlistenFns.push(fn))

    // 处理阶段状态
    listen<string>('process-status', (event) => {
      try {
        const data = JSON.parse(event.payload)
        console.log(`[处理] ${data.file_id} 阶段: ${data.stage}`)
      } catch {}
    }).then(fn => unlistenFns.push(fn))
  }

  setupEventListeners()

  return {
    files, selectedFileId, polishEnabledFileId, isPolishEnabled,
    isSettingsOpen, isTranscribing, apiConfig, statusInfo,
    selectedFile, completedCount, hasActiveFiles,
    theme, toastMessage, toastType, showToast,
    showToastMessage, loadConfig, saveConfig, setTheme, applyTheme,
    addFile, removeFile, updateFile, selectFile, clearCompleted,
    setPolishEnabled, resetStatus, ensureFileState, fileStates,
  }
})
