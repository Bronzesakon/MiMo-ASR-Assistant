// 设置 store（清单 F-04 + P3）：API 配置、主题、配置持久化、自定义服务商 CRUD。
// P3：保存队列（单写 Promise 串行化 + revision 乐观并发控制）、committed/draft 分离、
// dirtyFields/saving/saveError 状态、key 输入 trailing debounce、REVISION_CONFLICT 重试。
// F-06：提示词运行时职责已迁至 stores/prompts.ts（prompts.json 单一事实来源），
// 本 store 不再持有 polishPrompt；config.json.polish_prompt 仅后端迁移兼容保留。
import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type {
  ApiConfig,
  ConfigSnapshot,
  ConfigCommandError,
  CustomProvider,
  CustomModelOption,
} from '../types'

export const PROVIDER_URLS: Record<string, string> = {
  'mimo-api': 'https://api.xiaomimimo.com/v1',
  'mimo-token-plan': 'https://token-plan-cn.xiaomimimo.com/v1',
  'deepseek': 'https://api.deepseek.com',
}

const BUILTIN_POLISH_PROVIDERS = ['deepseek', 'mimo-api', 'mimo-token-plan']

/** 内置规整服务商的合法模型集合（H-03 归一基准，与后端 provider 定义一致） */
const BUILTIN_POLISH_MODELS: Record<string, string[]> = {
  'deepseek': ['deepseek-v4-pro', 'deepseek-v4-flash'],
  'mimo-api': ['mimo-v2.5', 'mimo-v2.5-pro'],
  'mimo-token-plan': ['mimo-v2.5', 'mimo-v2.5-pro'],
}

const DEFAULT_POLISH_PROVIDER = 'deepseek'
const DEFAULT_POLISH_MODEL = 'mimo-v2.5'

/** 需要落盘的配置字段（用于 dirtyFields 追踪与提交后回填 draft） */
type FieldKey =
  | 'theme'
  | 'sliceSeconds'
  | 'asrProvider'
  | 'polishSelection'
  | 'providerKey'
  | 'customProviders'
  | 'loggingEnabled'

function defaultConfig(): ApiConfig {
  return {
    provider: 'mimo-api',
    baseUrl: 'https://api.xiaomimimo.com/v1',
    apiKey: '',
    transcriptionModel: 'mimo-v2.5-asr',
    polishModel: 'mimo-v2.5',
    providerKeys: {},
    loggingEnabled: true,
    asrProvider: 'mimo-api',
    polishProvider: 'deepseek',
    sliceSeconds: 10,
    customProviders: [],
  }
}

function cloneConfig(cfg: ApiConfig): ApiConfig {
  return {
    ...cfg,
    providerKeys: { ...cfg.providerKeys },
    customProviders: cfg.customProviders.map(cp => ({
      ...cp,
      models: [...cp.models],
    })),
  }
}

/** 把后端 ConfigSnapshot（扁平 snake_case config）映射为 ApiConfig */
function fromSnapshot(snap: ConfigSnapshot): ApiConfig {
  return {
    provider: snap.provider,
    baseUrl: snap.base_url,
    apiKey: snap.api_key,
    transcriptionModel: snap.transcription_model,
    polishModel: snap.polish_model,
    providerKeys: snap.provider_keys,
    loggingEnabled: snap.logging_enabled,
    asrProvider: snap.asr_provider,
    polishProvider: snap.polish_provider,
    sliceSeconds: snap.slice_seconds,
    customProviders: snap.custom_providers || [],
  }
}

/** 解析结构化配置错误（识别 REVISION_CONFLICT / currentRevision） */
function parseConfigError(e: unknown): ConfigCommandError | null {
  if (e && typeof e === 'object') {
    const obj = e as Record<string, unknown>
    if (typeof obj.code === 'string') {
      return {
        code: obj.code,
        message: String(obj.message ?? ''),
        currentRevision: obj.currentRevision as number | undefined,
      }
    }
  }
  const s = String(e)
  if (s.includes('REVISION_CONFLICT')) {
    const m = s.match(/currentRevision[^0-9]*(\d+)/)
    return {
      code: 'REVISION_CONFLICT',
      message: s,
      currentRevision: m ? Number(m[1]) : undefined,
    }
  }
  return { code: 'UNKNOWN', message: s }
}

export const useSettingsStore = defineStore('settings', () => {
  // ---- committed（运行事实来源，任务启动用此快照，不使用未落盘草稿）----
  const committedConfig = ref<ApiConfig>(defaultConfig())
  const committedTheme = ref<'system' | 'light' | 'dark'>('system')
  const revision = ref(0)

  // ---- draft（界面编辑态）----
  const apiConfig = ref<ApiConfig>(defaultConfig())
  const theme = ref<'system' | 'light' | 'dark'>('system')

  // ---- 保存状态 ----
  const dirtyFields = ref<Set<FieldKey>>(new Set())
  const dirtyKeys = ref<Set<string>>(new Set())
  const saving = ref(false)
  const saveError = ref<string | null>(null)

  // ---- 单写队列（串行化所有 mutation，基于 revision 乐观并发控制）----
  let queue: Promise<unknown> = Promise.resolve()
  let keyDebounce: ReturnType<typeof setTimeout> | null = null
  let pendingKeyCommit: string | null = null // providerId
  const KEY_DEBOUNCE_MS = 400

  const asrKey = computed(() => committedConfig.value.providerKeys[committedConfig.value.asrProvider] || '')
  const asrBaseUrl = computed(() => PROVIDER_URLS[committedConfig.value.asrProvider] || PROVIDER_URLS['mimo-api'])

  function committedCustom(id: string): CustomProvider | null {
    if (!id.startsWith('custom_')) return null
    const pid = id.slice('custom_'.length)
    return committedConfig.value.customProviders.find(p => p.id === pid) || null
  }

  const polishKey = computed(() => {
    const cp = committedCustom(committedConfig.value.polishProvider)
    if (cp) return cp.apiKey
    return committedConfig.value.providerKeys[committedConfig.value.polishProvider] || ''
  })
  const polishBaseUrl = computed(() => {
    const cp = committedCustom(committedConfig.value.polishProvider)
    if (cp) return cp.baseUrl
    return PROVIDER_URLS[committedConfig.value.polishProvider] || PROVIDER_URLS['deepseek']
  })

  // ============================================================
  // 队列与提交
  // ============================================================

  function enqueue(task: () => Promise<void>): Promise<void> {
    const run = queue.then(task)
    queue = run.then(() => {}, () => {}) // 队列尾部吞掉错误，避免中断后续
    return run
  }

  /** 队列尾部：等待所有排队的 mutation 完成（含 key debounce flush） */
  async function flush(): Promise<void> {
    flushKeyCommit()
    await queue.catch(() => {})
  }

  /** 提交一个操作型命令：成功用快照回填 committed/draft；REVISION_CONFLICT 刷新重试一次 */
  async function commit(
    cmd: string,
    args: Record<string, unknown>,
    affected: FieldKey[],
    retried = false,
  ): Promise<void> {
    saving.value = true
    saveError.value = null
    try {
      const snap = await invoke<ConfigSnapshot>(cmd, {
        ...args,
        expectedRevision: revision.value,
      } as never)
      applySnapshot(snap, affected)
    } catch (e) {
      const parsed = parseConfigError(e)
      if (!retried && parsed?.code === 'REVISION_CONFLICT' && parsed.currentRevision != null) {
        revision.value = parsed.currentRevision
        return commit(cmd, args, affected, true)
      }
      saveError.value = parsed?.message || String(e)
      console.error(`[配置] ${cmd} 失败:`, e)
      throw e
    } finally {
      saving.value = false
    }
  }

  /** mutation 成功后用后端快照替换 committed 与相关 draft 字段 */
  function applySnapshot(snap: ConfigSnapshot, affected: FieldKey[]) {
    committedConfig.value = fromSnapshot(snap)
    revision.value = snap.revision
    committedTheme.value = (snap.theme as 'system' | 'light' | 'dark') || 'system'

    for (const f of affected) {
      if (f === 'theme') {
        theme.value = committedTheme.value
      } else if (f === 'providerKey') {
        apiConfig.value.providerKeys = { ...committedConfig.value.providerKeys }
        dirtyKeys.value.clear()
      } else if (f === 'customProviders') {
        apiConfig.value.customProviders = committedConfig.value.customProviders.map(cp => ({ ...cp, models: [...cp.models] }))
      } else if (f === 'polishSelection') {
        apiConfig.value.polishProvider = committedConfig.value.polishProvider
        apiConfig.value.polishModel = committedConfig.value.polishModel
      } else {
        // sliceSeconds / asrProvider / loggingEnabled 直接拷贝
        const key = f as unknown as keyof ApiConfig
        ;(apiConfig.value as Record<string, unknown>)[key] =
          (committedConfig.value as unknown as Record<string, unknown>)[key]
      }
      dirtyFields.value.delete(f)
    }
  }

  // ============================================================
  // 配置加载
  // ============================================================

  async function loadConfig() {
    try {
      const snap = await invoke<ConfigSnapshot>('load_config')
      committedConfig.value = fromSnapshot(snap)
      revision.value = snap.revision
      committedTheme.value = (snap.theme as 'system' | 'light' | 'dark') || 'system'
      theme.value = committedTheme.value
      apiConfig.value = cloneConfig(committedConfig.value)
      dirtyFields.value.clear()
      dirtyKeys.value.clear()
      saveError.value = null
      applyTheme()
    } catch (e) {
      console.error('[配置] 加载失败:', e)
    }
  }

  // ============================================================
  // 主题
  // ============================================================

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
    dirtyFields.value.add('theme')
    enqueue(() => commit('set_theme', { theme: t }, ['theme']))
  }

  if (typeof window !== 'undefined') {
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
      if (theme.value === 'system') applyTheme()
    })
  }

  // ============================================================
  // 操作型 mutation（P3：全部进入单写队列）
  // ============================================================

  function setSliceSeconds(seconds: number) {
    apiConfig.value.sliceSeconds = seconds
    dirtyFields.value.add('sliceSeconds')
    enqueue(() => commit('set_slice_seconds', { seconds }, ['sliceSeconds']))
  }

  function setAsrProvider(providerId: string) {
    apiConfig.value.asrProvider = providerId
    dirtyFields.value.add('asrProvider')
    enqueue(() => commit('set_asr_provider', { providerId }, ['asrProvider']))
  }

  function setPolishSelection(provider: string, model: string) {
    apiConfig.value.polishProvider = provider
    apiConfig.value.polishModel = model
    dirtyFields.value.add('polishSelection')
    enqueue(() => commit('set_polish_selection', { providerId: provider, modelId: model }, ['polishSelection']))
  }

  /** key 输入：trailing debounce，离开输入框/关闭面板时 flush 立即提交 */
  function setProviderKeyInput(providerId: string, key: string) {
    apiConfig.value.providerKeys[providerId] = key
    pendingKeyCommit = providerId
    dirtyKeys.value.add(providerId)
    if (keyDebounce) clearTimeout(keyDebounce)
    keyDebounce = setTimeout(() => {
      keyDebounce = null
      flushKeyCommit()
    }, KEY_DEBOUNCE_MS)
  }

  function flushKeyCommit() {
    if (keyDebounce) { clearTimeout(keyDebounce); keyDebounce = null }
    if (!pendingKeyCommit) return
    const providerId = pendingKeyCommit
    pendingKeyCommit = null
    const key = apiConfig.value.providerKeys[providerId] || ''
    enqueue(() => commit('set_provider_key', { providerId, key }, ['providerKey']))
  }

  function setLoggingEnabled(enabled: boolean) {
    apiConfig.value.loggingEnabled = enabled
    dirtyFields.value.add('loggingEnabled')
    enqueue(() => commit('set_logging_enabled', { enabled }, ['loggingEnabled']))
  }

  // ============================================================
  // 自定义服务商 CRUD（P3：走操作型命令）
  // ============================================================

  function upsertCustomProvider(provider: CustomProvider) {
    const idx = apiConfig.value.customProviders.findIndex(p => p.id === provider.id)
    if (idx > -1) {
      apiConfig.value.customProviders[idx] = { ...provider, models: [...provider.models] }
    } else {
      apiConfig.value.customProviders.push({ ...provider, models: [...provider.models] })
    }
    dirtyFields.value.add('customProviders')
    enqueue(() =>
      commit('upsert_custom_provider',
        {
          provider: {
            id: provider.id,
            name: provider.name,
            baseUrl: provider.baseUrl,
            apiKey: provider.apiKey,
            models: provider.models,
          },
        },
        ['customProviders'],
      ),
    )
  }

  function addCustomProvider(provider: CustomProvider) { upsertCustomProvider(provider) }

  function updateCustomProvider(id: string, updates: Partial<CustomProvider>) {
    const cp = apiConfig.value.customProviders.find(p => p.id === id)
    const merged: CustomProvider = cp
      ? { ...cp, ...updates, models: [...(updates.models ?? cp.models)] }
      : {
          id,
          name: updates.name ?? '',
          baseUrl: updates.baseUrl ?? '',
          apiKey: updates.apiKey ?? '',
          models: [...(updates.models ?? [])],
        }
    upsertCustomProvider(merged)
  }

  function removeCustomProvider(id: string) {
    apiConfig.value.customProviders = apiConfig.value.customProviders.filter(p => p.id !== id)
    // H-03：删除后归一 polish provider/model（含删除当前选中自定义服务商的场景）
    const { provider, model } = normalizePolish(apiConfig.value.polishProvider, apiConfig.value.polishModel)
    apiConfig.value.polishProvider = provider
    apiConfig.value.polishModel = model
    dirtyFields.value.add('customProviders')
    enqueue(() => commit('delete_custom_provider', { providerId: id }, ['customProviders']))
  }

  // ============================================================
  // 重试（PGC 冲突或失败后，重放 dirty 字段）
  // ============================================================

  /** 重放所有未落盘字段（设置抽屉"重试"入口） */
  function retryAll() {
    if (dirtyFields.value.has('theme')) {
      enqueue(() => commit('set_theme', { theme: theme.value }, ['theme']))
    }
    if (dirtyFields.value.has('sliceSeconds')) {
      enqueue(() => commit('set_slice_seconds', { seconds: apiConfig.value.sliceSeconds }, ['sliceSeconds']))
    }
    if (dirtyFields.value.has('asrProvider')) {
      enqueue(() => commit('set_asr_provider', { providerId: apiConfig.value.asrProvider }, ['asrProvider']))
    }
    if (dirtyFields.value.has('polishSelection')) {
      enqueue(() => commit('set_polish_selection', { providerId: apiConfig.value.polishProvider, modelId: apiConfig.value.polishModel }, ['polishSelection']))
    }
    if (dirtyFields.value.has('loggingEnabled')) {
      enqueue(() => commit('set_logging_enabled', { enabled: apiConfig.value.loggingEnabled }, ['loggingEnabled']))
    }
    for (const providerId of dirtyKeys.value) {
      enqueue(() => commit('set_provider_key', { providerId, key: apiConfig.value.providerKeys[providerId] || '' }, ['providerKey']))
    }
    if (dirtyFields.value.has('customProviders')) {
      for (const cp of apiConfig.value.customProviders) {
        enqueue(() =>
          commit('upsert_custom_provider',
            { provider: { id: cp.id, name: cp.name, baseUrl: cp.baseUrl, apiKey: cp.apiKey, models: cp.models } },
            ['customProviders'],
          ),
        )
      }
    }
  }

  // ============================================================
  // 自定义服务商辅助（作用于 draft，供设置抽屉使用）
  // ============================================================

  function isCustomProvider(id: string): boolean {
    return id.startsWith('custom_')
  }

  function getCustomProvider(id: string): CustomProvider | null {
    if (!isCustomProvider(id)) return null
    const providerId = id.slice('custom_'.length)
    return apiConfig.value.customProviders.find(p => p.id === providerId) || null
  }

  /**
   * H-03：归一 polish provider/model 组合。
   * provider 指向已删除的自定义服务商时回退内置默认；
   * 模型必须属于目标 provider 合法集合，否则取集合首个；集合为空取默认模型。
   */
  function normalizePolish(provider: string, model: string): { provider: string; model: string } {
    let allowed: string[]
    let resolvedProvider = provider
    if (isCustomProvider(provider)) {
      const cp = getCustomProvider(provider)
      if (cp) {
        allowed = cp.models
      } else {
        resolvedProvider = DEFAULT_POLISH_PROVIDER
        allowed = BUILTIN_POLISH_MODELS[resolvedProvider] || []
      }
    } else {
      allowed = BUILTIN_POLISH_MODELS[provider] || []
      if (allowed.length === 0) {
        resolvedProvider = DEFAULT_POLISH_PROVIDER
        allowed = BUILTIN_POLISH_MODELS[resolvedProvider] || []
      }
    }
    const resolvedModel = allowed.includes(model) ? model : allowed[0] || DEFAULT_POLISH_MODEL
    return { provider: resolvedProvider, model: resolvedModel }
  }

  const polishProviderOptions = computed(() => {
    const options: CustomModelOption[] = []
    const names: Record<string, string> = {
      'deepseek': 'DeepSeek',
      'mimo-api': 'MiMo API',
      'mimo-token-plan': 'MiMo Token Plan',
    }
    for (const id of BUILTIN_POLISH_PROVIDERS) {
      options.push({ type: 'builtin', providerId: id, modelId: id, label: names[id] || id })
    }
    for (const cp of apiConfig.value.customProviders) {
      options.push({ type: 'custom', providerId: cp.id, modelId: cp.id, label: cp.name, customProviderId: cp.id })
    }
    return options
  })

  const customPolishModels = computed(() => {
    const cp = getCustomProvider(apiConfig.value.polishProvider)
    if (!cp) return []
    return cp.models
  })

  function getCustomOptionValue(customProviderId: string): string {
    return `custom_${customProviderId}`
  }

  return {
    // committed（运行事实来源）
    committedConfig, committedTheme, revision,
    asrKey, asrBaseUrl, polishKey, polishBaseUrl,
    // draft（编辑态）
    apiConfig, theme,
    // 保存状态
    dirtyFields, dirtyKeys, saving, saveError,
    loadConfig, applyTheme, setTheme,
    setSliceSeconds, setAsrProvider, setPolishSelection,
    setProviderKeyInput, flushKeyCommit, setLoggingEnabled,
    addCustomProvider, updateCustomProvider, removeCustomProvider,
    retryAll, flush,
    polishProviderOptions, customPolishModels,
    getCustomOptionValue, isCustomProvider, getCustomProvider, normalizePolish,
  }
})