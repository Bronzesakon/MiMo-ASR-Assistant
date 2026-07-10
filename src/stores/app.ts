import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { FileInfo, ApiConfig, CustomProvider, CustomModelOption } from '../types'
import { needsMarkdownParsing } from '../utils/markdown'

const PROVIDER_URLS: Record<string, string> = {
  'mimo-api': 'https://api.xiaomimimo.com/v1',
  'mimo-token-plan': 'https://token-plan-cn.xiaomimimo.com/v1',
  'deepseek': 'https://api.deepseek.com',
}

interface RustConfig {
  provider: string
  base_url: string
  api_key: string
  transcription_model: string
  polish_model: string
  theme: string
  polish_prompt: string
  provider_keys: Record<string, string>
  logging_enabled: boolean
  asr_provider: string
  polish_provider: string
  slice_seconds: number
  custom_providers: Array<{ id: string; name: string; base_url: string; api_key: string; models: string[] }>
}

interface FileProcessState {
  transcribeStartTime: number
  outputCharCount: number
  totalSlices: number
  currentSlice: number
  transcriptionChunks: number
  polishChunkCount: number
  polishFirstChunk: boolean
}

export const useAppStore = defineStore('app', () => {
  const files = ref<FileInfo[]>([])
  const selectedFileId = ref<string | null>(null)

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
    polishPrompt: '给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，优化每句话的口语化表达的语气词、重复字等，要求只做修饰，每句话都要输出，不可简略，不可概括，根据原文的内容，智能切分长段落',
    providerKeys: {},
    loggingEnabled: true,
    asrProvider: 'mimo-api',
    polishProvider: 'deepseek',
    sliceSeconds: 10,
    customProviders: [],
  })

  const fileStates = new Map<string, FileProcessState>()

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
      modelName: apiConfig.value.transcriptionModel,
      outputSpeed: totalOutputSpeed,
      tokenCount: globalTokenCount.value,
    }
  })

  const globalTokenCount = ref(0)
  const speedTick = ref(0)

  // 渲染节流：流式 chunk 到达时不立即触发渲染，等待 150ms 静默后再重绘
  const transcriptionRenderTick = ref(0)
  const polishRenderTick = ref(0)
  const DEBOUNCE_MS = 150
  const renderTimers = new Map<string, ReturnType<typeof setTimeout>>()
  // upgrade-only 标记：一旦检测到 Markdown 语法，本文件本次转写/规整锁定为 markdown 模式
  const fileRenderMode = new Map<string, 'plain' | 'markdown'>()

  function scheduleRender(fileId: string, type: 'transcription' | 'polish', delta: string) {
    const mode = fileRenderMode.get(fileId) || 'plain'
    if (mode === 'plain' && needsMarkdownParsing(delta)) {
      fileRenderMode.set(fileId, 'markdown')
    }
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

  const asrKey = computed(() => apiConfig.value.providerKeys[apiConfig.value.asrProvider] || '')
  const asrBaseUrl = computed(() => PROVIDER_URLS[apiConfig.value.asrProvider] || PROVIDER_URLS['mimo-api'])

  // Custom provider 辅助 - polishProvider 格式：custom_{providerId}
  function isCustomProvider(id: string): boolean {
    return id.startsWith('custom_')
  }

  function getCustomProvider(id: string): CustomProvider | null {
    if (!isCustomProvider(id)) return null
    const providerId = id.slice('custom_'.length)
    return apiConfig.value.customProviders.find(p => p.id === providerId) || null
  }

  const polishKey = computed(() => {
    const cp = getCustomProvider(apiConfig.value.polishProvider)
    if (cp) return cp.apiKey
    return apiConfig.value.providerKeys[apiConfig.value.polishProvider] || ''
  })
  const polishBaseUrl = computed(() => {
    const cp = getCustomProvider(apiConfig.value.polishProvider)
    if (cp) return cp.baseUrl
    return PROVIDER_URLS[apiConfig.value.polishProvider] || PROVIDER_URLS['deepseek']
  })
  const polishModel = computed(() => {
    if (isCustomProvider(apiConfig.value.polishProvider)) {
      return apiConfig.value.polishModel
    }
    return apiConfig.value.polishModel
  })

  function showToastMessage(message: string, type: 'info' | 'error' | 'success' = 'info') {
    toastMessage.value = message
    toastType.value = type
    showToast.value = true
    setTimeout(() => { showToast.value = false }, 3000)
  }

  const sliceOptionValues = [10, 30, 60, 150]

  async function loadConfig() {
    try {
      const config: RustConfig = await invoke('load_config')
      const rawSlice = config.slice_seconds || 10
      const snappedSlice = sliceOptionValues.reduce((prev, cur) =>
        Math.abs(cur - rawSlice) < Math.abs(prev - rawSlice) ? cur : prev
      )
      apiConfig.value = {
        provider: config.provider,
        baseUrl: config.base_url,
        apiKey: config.api_key,
        transcriptionModel: config.transcription_model || 'mimo-v2.5-asr',
        polishModel: config.polish_model || 'mimo-v2.5',
        polishPrompt: config.polish_prompt || '给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，优化每句话的口语化表达的语气词、重复字等，要求只做修饰，每句话都要输出，不可简略，不可概括，根据原文的内容，智能切分长段落',
        providerKeys: config.provider_keys || {},
        loggingEnabled: config.logging_enabled ?? true,
        asrProvider: config.asr_provider || 'mimo-api',
        polishProvider: config.polish_provider || 'deepseek',
        sliceSeconds: snappedSlice,
        customProviders: (config.custom_providers || []).map(cp => ({
          id: cp.id, name: cp.name, baseUrl: cp.base_url, apiKey: cp.api_key, models: cp.models,
        })),
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
          logging_enabled: apiConfig.value.loggingEnabled,
          asr_provider: apiConfig.value.asrProvider,
          polish_provider: apiConfig.value.polishProvider,
          slice_seconds: apiConfig.value.sliceSeconds || 10,
          custom_providers: apiConfig.value.customProviders.map(cp => ({
            id: cp.id, name: cp.name, base_url: cp.baseUrl, api_key: cp.apiKey, models: cp.models,
          })),
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
    if (files.value.some(f => f.path === file.path)) {
      showToastMessage(`${file.name} 已在列表中`, 'info')
      return
    }
    files.value.push(file)
    if (!selectedFileId.value) selectedFileId.value = file.id
    console.log(`[文件] 导入: ${file.name}, 时长=${file.duration.toFixed(1)}s, id=${file.id}`)
  }

  function removeFile(id: string) {
    const index = files.value.findIndex(f => f.id === id)
    if (index > -1) {
      files.value.splice(index, 1)
      fileStates.delete(id)
      fileRenderMode.delete(id)
      flushRender(id)
      if (selectedFileId.value === id) selectedFileId.value = files.value[0]?.id || null
    }
  }

  function updateFile(id: string, updates: Partial<FileInfo>) {
    const file = files.value.find(f => f.id === id)
    if (file) Object.assign(file, updates)
  }

  function selectFile(id: string) { selectedFileId.value = id }

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
        totalSlices: 0, currentSlice: 0,
        transcriptionChunks: 0,
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
        if (!file) return
        if (file) {
          file.transcription += text
          file.transcriptionWordCount = countWords(file.transcription)
        }
        const state = ensureFileState(fileId)
        state.outputCharCount += text.length
        state.transcriptionChunks++
        scheduleRender(fileId, 'transcription', text)
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
        }
        // 进度 = 已完成分片 / 总分片数（纯 API 进度，不含切片）
        if (file && state.totalSlices > 0) {
          file.progress = Math.min(97, Math.round(state.currentSlice / state.totalSlices * 97))
          // 第一个 progress 事件到达 → 从"准备中"切换到"转写中"
          if (file.status === 'processing') {
            file.status = 'transcribing'
          }
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
        flushRender(fileId)
        isTranscribing.value = files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 转写完成`, 'success')
      } catch (e) {
        console.error('[事件] transcription-complete 解析失败:', e)
      }
    }).then(fn => unlistenFns.push(fn))

    // 转写暂停
    listen<string>('transcription-paused', (event) => {
      try {
        const data = JSON.parse(event.payload)
        const fileId = data.file_id
        const file = files.value.find(f => f.id === fileId)
        if (file) {
          file.status = 'paused'
          file.tempDir = data.temp_dir
          file.lastCompletedIndex = data.last_completed
          file.resumeTotal = data.total
          file.transcriptionWordCount = countWords(file.transcription)
        }
        flushRender(fileId)
        // 不删除 fileStates：保留 transcriptionChunks 等状态供 resume 时进度计算
        isTranscribing.value = files.value.some(f =>
          f.status === 'transcribing' || f.status === 'processing' || f.status === 'pausing'
        )
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 已暂停转写`, 'info')
      } catch (e) {
        console.error('[事件] transcription-paused 解析失败:', e)
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
        if (!file) return
        if (file) {
          file.polished += text
          file.polishedWordCount = countWords(file.polished)
        }
        const state = ensureFileState(fileId)
        if (!state.polishFirstChunk) {
          state.polishFirstChunk = true
          if (file) updateFile(fileId, { polishStage: 'receiving' })
        }
        state.polishChunkCount++
        scheduleRender(fileId, 'polish', text)
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
          file.polishStage = 'idle'
          console.log(`[规整] 完成: ${file.name}, 字数=${file.polishedWordCount}`)
        }
        flushRender(fileId)
        if (fileId === selectedFileId.value) showToastMessage(`${file?.name || '文件'} 规整完成`, 'success')
      } catch (e) {
        console.error('[事件] chat-complete 解析失败:', e)
      }
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

    // 每秒刷新输出速度
    setInterval(() => {
      if (files.value.some(f => f.status === 'transcribing' || f.status === 'processing')) {
        speedTick.value++
      }
    }, 1000)
  }

  setupEventListeners()

  function requestCancelFile(fileId: string) {
    const file = files.value.find(f => f.id === fileId)
    invoke('cancel_task', { fileId, fileName: file?.name || fileId }).catch(() => {})
  }

  function pauseTranscription(fileId: string) {
    const file = files.value.find(f => f.id === fileId)
    if (!file) return
    updateFile(fileId, { status: 'pausing' })
    showToastMessage(`${file.name} 正在暂停转写`, 'info')
    invoke('pause_task', { fileId, fileName: file.name }).catch(() => {})
  }

  async function resumeTranscription(fileId: string) {
    const file = files.value.find(f => f.id === fileId)
    if (!file || !file.tempDir || !file.lastCompletedIndex) return

    const startIndex = (file.lastCompletedIndex ?? 0) + 1
    const resumeTotal = file.resumeTotal ?? 1
    updateFile(fileId, { status: 'processing', progress: 0 })
    isTranscribing.value = true

    const state = ensureFileState(fileId)
    state.transcribeStartTime = Date.now()
    state.outputCharCount = 0
    state.totalSlices = resumeTotal
    state.currentSlice = startIndex - 1
    // transcriptionChunks 不重置：保留已累积的文本进度，防止进度回退

    try {
      await invoke('resume_transcription', {
        fileId,
        fileName: file.name,
        tempDir: file.tempDir,
        startIndex: file.lastCompletedIndex + 1,
        apiParams: {
          base_url: asrBaseUrl.value,
          api_key: asrKey.value,
          model: apiConfig.value.transcriptionModel,
          language: file.language || 'auto',
        },
      })
    } catch (e) {
      console.error(`[转写] 继续转写失败: ${file.name}`, e)
      const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
      updateFile(fileId, { status: 'failed', error: errMsg })
      showToastMessage(`${file.name} 继续转写失败`, 'error')
    }

    isTranscribing.value = files.value.some(f => f.status === 'transcribing' || f.status === 'processing')
  }

  // ============================================================
  // 自定义服务商 CRUD
  // ============================================================

  const BUILTIN_POLISH_PROVIDERS = ['deepseek', 'mimo-api', 'mimo-token-plan']

  function addCustomProvider(provider: CustomProvider) {
    apiConfig.value.customProviders.push(provider)
    saveConfig()
  }

  function updateCustomProvider(id: string, updates: Partial<CustomProvider>) {
    const cp = apiConfig.value.customProviders.find(p => p.id === id)
    if (cp) Object.assign(cp, updates)
    saveConfig()
  }

  function removeCustomProvider(id: string) {
    apiConfig.value.customProviders = apiConfig.value.customProviders.filter(p => p.id !== id)
    // 如果当前选中的 polishProvider 属于被删除的自定义服务商，重置为 deepseek
    if (isCustomProvider(apiConfig.value.polishProvider)) {
      const providerId = apiConfig.value.polishProvider.slice('custom_'.length)
      if (providerId === id) {
        apiConfig.value.polishProvider = 'deepseek'
      }
    }
    saveConfig()
  }

  const polishProviderOptions = computed(() => {
    const options: CustomModelOption[] = []
    // 内置服务商
    for (const id of BUILTIN_POLISH_PROVIDERS) {
      const names: Record<string, string> = {
        'deepseek': 'DeepSeek',
        'mimo-api': 'MiMo API',
        'mimo-token-plan': 'MiMo Token Plan',
      }
      options.push({ type: 'builtin', providerId: id, modelId: id, label: names[id] || id })
    }
    // 自定义服务商 - 只显示名称，不显示模型
    for (const cp of apiConfig.value.customProviders) {
      options.push({
        type: 'custom',
        providerId: cp.id,
        modelId: cp.id,
        label: cp.name,
        customProviderId: cp.id,
      })
    }
    return options
  })

  // 当前选中的自定义服务商的模型列表
  const customPolishModels = computed(() => {
    const cp = getCustomProvider(apiConfig.value.polishProvider)
    if (!cp) return []
    return cp.models
  })

  /** 获取自定义服务商选项的 value 编码 */
  function getCustomOptionValue(customProviderId: string): string {
    return `custom_${customProviderId}`
  }

  return {
    files, selectedFileId,
    isSettingsOpen, isTranscribing, apiConfig, statusInfo,
    selectedFile, completedCount, hasActiveFiles,
    asrKey, polishKey, asrBaseUrl, polishBaseUrl, polishModel,
    theme, toastMessage, toastType, showToast,
    showToastMessage, loadConfig, saveConfig, setTheme, applyTheme,
    addFile, removeFile, updateFile, selectFile,
    resetStatus, ensureFileState, fileStates,
    requestCancelFile, speedTick,
    pauseTranscription, resumeTranscription,
    addCustomProvider, updateCustomProvider, removeCustomProvider,
    polishProviderOptions, customPolishModels,
    getCustomOptionValue, isCustomProvider, getCustomProvider,
    transcriptionRenderTick, polishRenderTick,
    fileRenderMode, scheduleRender, flushRender,
  }
})
