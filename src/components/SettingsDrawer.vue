<script setup lang="ts">
import { useSettingsStore } from '../stores/settings'
import { useFilesStore } from '../stores/files'
import { useUiStore } from '../stores/ui'
import { invoke } from '@tauri-apps/api/core'
import { ref, computed, reactive, watch } from 'vue'
import { openExternalUrl } from '../utils/externalLink'
import CustomSelect from './CustomSelect.vue'
import CustomProviderModal from './CustomProviderModal.vue'
import PromptManager from './PromptManager.vue'
import { useCapabilitiesStore } from '../stores/capabilities'
import type { CustomProvider } from '../types'

const store = useSettingsStore()
const files = useFilesStore()
const ui = useUiStore()
const caps = useCapabilitiesStore()
const promptManagerRef = ref<InstanceType<typeof PromptManager> | null>(null)
const showAsrKey = ref(false)
const showPolishKey = ref(false)
const showHelp = reactive<Record<string, boolean>>({ asrKey: false, polishKey: false })

const customModalVisible = ref(false)
const customModalEdit = ref<CustomProvider | null>(null)

function toggleHelp(id: string) {
  showHelp[id] = !showHelp[id]
}

interface HelpLink { label: string; href: string }

const asrProviders = [
  { id: 'mimo-api', name: 'MiMo API', url: 'https://api.xiaomimimo.com/v1', prefix: 'sk-', keyLabel: 'MiMo API Key' },
  { id: 'mimo-token-plan', name: 'MiMo Token Plan', url: 'https://token-plan-cn.xiaomimimo.com/v1', prefix: 'tp-', keyLabel: 'MiMo Token Plan Key' },
]

const polishProviders = [
  { id: 'deepseek', name: 'DeepSeek', url: 'https://api.deepseek.com', prefix: 'sk-', keyLabel: 'DeepSeek API Key', chat: ['deepseek-v4-pro', 'deepseek-v4-flash'] },
  { id: 'mimo-api', name: 'MiMo API', url: 'https://api.xiaomimimo.com/v1', prefix: 'sk-', keyLabel: 'MiMo API Key', chat: ['mimo-v2.5', 'mimo-v2.5-pro'] },
  { id: 'mimo-token-plan', name: 'MiMo Token Plan', url: 'https://token-plan-cn.xiaomimimo.com/v1', prefix: 'tp-', keyLabel: 'MiMo Token Plan Key', chat: ['mimo-v2.5', 'mimo-v2.5-pro'] },
]

const currentAsrProv = computed(() => asrProviders.find(p => p.id === store.apiConfig.asrProvider) || asrProviders[0])
const currentPolishProv = computed(() => polishProviders.find(p => p.id === store.apiConfig.polishProvider) || polishProviders[0])
const isCustomPolishProvider = computed(() => store.isCustomProvider(store.apiConfig.polishProvider))
const currentCustomPolishProv = computed(() => {
  return store.getCustomProvider(store.apiConfig.polishProvider)
})

function getAsrHelpLinks(): { text: string; links: HelpLink[] } {
  const id = store.apiConfig.asrProvider
  if (id === 'mimo-token-plan') return {
    text: 'MiMo Token Plan 为订阅制服务，API Key 是调用服务的身份凭证。前往 {0} 创建 API Key，{1} 查看额度。',
    links: [
      { label: 'Token Plan 管理', href: 'https://platform.xiaomimimo.com/console/plan-manage' },
      { label: 'MiMo 控制台', href: 'https://platform.xiaomimimo.com/console/balance' }
    ]
  }
  return {
    text: 'MiMo API 为按量付费服务，API Key 是调用服务的身份凭证。前往 {0} 创建 API Key，{1} 查看余额。',
    links: [
      { label: 'MiMo API Keys', href: 'https://platform.xiaomimimo.com/console/api-keys' },
      { label: 'MiMo 控制台', href: 'https://platform.xiaomimimo.com/console/balance' }
    ]
  }
}

function getPolishHelpLinks(): { text: string; links: HelpLink[] } {
  const id = store.apiConfig.polishProvider
  if (id === 'mimo-api') return {
    text: 'MiMo API 为按量付费服务，API Key 是调用服务的身份凭证。前往 {0} 创建 API Key，{1} 查看余额。',
    links: [
      { label: 'MiMo API Keys', href: 'https://platform.xiaomimimo.com/console/api-keys' },
      { label: 'MiMo 控制台', href: 'https://platform.xiaomimimo.com/console/balance' }
    ]
  }
  if (id === 'mimo-token-plan') return {
    text: 'MiMo Token Plan 为订阅制服务，API Key 是调用服务的身份凭证。前往 {0} 创建 API Key，{1} 查看额度。',
    links: [
      { label: 'Token Plan 管理', href: 'https://platform.xiaomimimo.com/console/plan-manage' },
      { label: 'MiMo 控制台', href: 'https://platform.xiaomimimo.com/console/balance' }
    ]
  }
  return {
    text: 'DeepSeek API 为按量付费服务，API Key 是调用服务的身份凭证。前往 {0} 创建 API Key，{1} 为账户充值。',
    links: [
      { label: 'DeepSeek 控制台', href: 'https://platform.deepseek.com/api_keys' },
      { label: '充值页面', href: 'https://platform.deepseek.com/top_up' }
    ]
  }
}

/**
 * 帮助文案分段（清单 S-01）：文本与链接由模板循环渲染，
 * 不再拼接 HTML 字符串/使用 v-html；链接点击统一走外链白名单出口。
 */
interface HelpSegment {
  text: string
  link?: HelpLink
}

function splitHelp(data: { text: string; links: HelpLink[] }): HelpSegment[] {
  const segments: HelpSegment[] = []
  let rest = data.text
  for (let i = 0; i < data.links.length; i++) {
    const marker = `{${i}}`
    const idx = rest.indexOf(marker)
    if (idx === -1) continue
    segments.push({ text: rest.slice(0, idx) }, { text: data.links[i].label, link: data.links[i] })
    rest = rest.slice(idx + marker.length)
  }
  if (rest) segments.push({ text: rest })
  return segments
}

const asrHelpSegments = computed(() => splitHelp(getAsrHelpLinks()))
const polishHelpSegments = computed(() => splitHelp(getPolishHelpLinks()))

async function onOpenExternal(href: string) {
  // 白名单校验 + Tauri opener；非法 URL 静默拒绝（静态常量正常不会走到）
  await openExternalUrl(href)
}

function onToggleLogging(v: boolean) {
  store.setLoggingEnabled(v)
}

async function close() {
  // F-06.5.8：关闭前若存在未保存草稿，由 PromptManager 弹三选确认
  const mgr = promptManagerRef.value
  if (mgr && !(await mgr.requestClose())) return
  // P3：关闭前 await 保存队列；失败时保持打开并显示错误与重试入口
  await store.flush()
  if (store.saveError) {
    ui.showToastMessage('有配置未保存成功，请重试', 'error')
    return
  }
  ui.isSettingsOpen = false
}

// P8：切片选项来自能力文档（capabilities.sliceSeconds），缺失时兜底内置值
const sliceOptions = computed(() => {
  const values = caps.caps?.sliceSeconds?.length ? caps.caps.sliceSeconds : [10, 30, 60, 150]
  return values.map(v => ({ value: String(v), label: v === 10 ? '10 秒（默认）' : `${v} 秒` }))
})

watch(() => ui.isSettingsOpen, (v) => { if (v) void caps.ensureLoaded() })

const sliceDisabled = computed(() =>
  files.files.some(f => ['transcribing', 'processing', 'pausing', 'paused'].includes(f.status))
)

function onSliceSecondsChange(val: string) {
  store.setSliceSeconds(Number(val))
}

function onAsrProviderChange(v: string) {
  store.setAsrProvider(v)
}

function onPolishProviderChange(v: string) {
  store.apiConfig.polishProvider = v
  // H-03：复用 store 归一逻辑，保证模型属于目标 provider 合法集合
  const { model } = store.normalizePolish(v, store.apiConfig.polishModel)
  store.setPolishSelection(v, model)
}

function onPolishModelChange(v: string) {
  store.setPolishSelection(store.apiConfig.polishProvider, v)
}

function onCustomDelete(customProviderId: string) {
  store.removeCustomProvider(customProviderId)
}

function onCustomAdd() {
  customModalEdit.value = null
  customModalVisible.value = true
}

function onCustomEdit(customProviderId: string) {
  const cp = store.apiConfig.customProviders.find(p => p.id === customProviderId)
  if (cp) {
    customModalEdit.value = cp
    customModalVisible.value = true
  }
}

function onCustomModalSave(provider: CustomProvider) {
  if (customModalEdit.value) {
    store.updateCustomProvider(provider.id, provider)
    // 如果当前选中的模型不在新列表中，默认选中第一个
    if (store.apiConfig.polishProvider === `custom_${provider.id}` &&
        !provider.models.includes(store.apiConfig.polishModel)) {
      if (provider.models.length > 0) {
        store.apiConfig.polishModel = provider.models[0]
      }
    }
  } else {
    store.addCustomProvider(provider)
  }
  customModalVisible.value = false
  customModalEdit.value = null
}

function onCustomModalClose() {
  customModalVisible.value = false
  customModalEdit.value = null
}

// P3：key 输入 trailing debounce；离开输入框立即 flush
function onAsrKeyInput(e: Event) {
  store.setProviderKeyInput(store.apiConfig.asrProvider, (e.target as HTMLInputElement).value)
}

function onPolishKeyInput(e: Event) {
  store.setProviderKeyInput(store.apiConfig.polishProvider, (e.target as HTMLInputElement).value)
}

function onAsrKeyChange() {
  store.flushKeyCommit()
}

function onPolishKeyChange() {
  store.flushKeyCommit()
}

function onRetry() {
  store.retryAll()
  void store.flush()
}

function onOverlayKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}

function openLogDir() {
  invoke('open_log_dir').catch(() => {})
}

function openPromptDir() {
  invoke('open_prompt_dir').catch(() => {})
}
</script>

<template>
  <Transition name="modal">
    <div v-if="ui.isSettingsOpen" class="overlay" @click.self="close" @keydown="onOverlayKeydown" tabindex="-1">
      <div class="panel">
        <div class="panel-header">
          <span class="panel-title">设置</span>
          <button class="close-btn" @click="close">
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
        </div>

        <div class="panel-body">
          <div v-if="store.saveError" class="save-error">
            <span class="save-error-text">配置保存失败，请重试</span>
            <button class="key-toggle" @click="onRetry">重试</button>
          </div>

          <div class="field">
            <label class="label">主题</label>
            <div class="theme-row">
              <button v-for="t in ['system','light','dark'] as const" :key="t" class="theme-btn" :class="{ on: store.theme === t }" @click="store.setTheme(t)">
                {{ t === 'system' ? '跟随系统' : t === 'light' ? '亮色' : '暗色' }}
              </button>
            </div>
          </div>

          <!-- 转文字 -->
          <div class="section-divider"><span>转文字</span></div>

          <div class="field">
            <label class="label">转写模型</label>
            <input class="input input-readonly" value="MiMo-V2.5-ASR" readonly />
            <div class="hint-box">
              <p class="hint-text">固定使用 MiMo-V2.5-ASR 模型进行语音转写</p>
            </div>
          </div>

          <div class="field">
            <label class="label">转写服务商</label>
            <CustomSelect
              :model-value="store.apiConfig.asrProvider"
              :options="asrProviders.map(p => ({ value: p.id, label: p.name }))"
              @update:model-value="onAsrProviderChange"
            />
          </div>

          <div class="field">
            <label class="label">{{ currentAsrProv.keyLabel }}</label>
            <div class="key-row">
              <input
                :value="store.apiConfig.providerKeys[store.apiConfig.asrProvider] || ''"
                @input="onAsrKeyInput"
                :type="showAsrKey ? 'text' : 'password'"
                class="input"
                :placeholder="currentAsrProv.prefix + '...'"
                @change="onAsrKeyChange"
                @blur="onAsrKeyChange"
              />
              <button class="key-toggle" @click="showAsrKey = !showAsrKey">{{ showAsrKey ? '隐藏' : '显示' }}</button>
            </div>
            <button class="hint-trigger" @click="toggleHelp('asrKey')">这是什么？</button>
            <Transition name="hint-expand">
              <div v-if="showHelp.asrKey" class="hint-box">
                <p class="hint-text">
                  <template v-for="(seg, i) in asrHelpSegments" :key="i">
                    <a v-if="seg.link" href="#" class="hint-link" @click.prevent="onOpenExternal(seg.link.href)">{{ seg.text }}</a>
                    <template v-else>{{ seg.text }}</template>
                  </template>
                </p>
              </div>
            </Transition>
          </div>

          <div class="field">
            <label class="label">切片时长</label>
            <CustomSelect
              :model-value="String(store.apiConfig.sliceSeconds)"
              :options="sliceOptions"
              :disabled="sliceDisabled"
              @update:model-value="onSliceSecondsChange"
            />
            <div class="hint-box">
              <p class="hint-text">单次发送给 API 的音频时长。越短响应越快，越长分片越少。有任务进行或暂停时不可修改。</p>
            </div>
          </div>

          <!-- 规整 -->
          <div class="section-divider"><span>规整</span></div>

          <div class="field">
            <label class="label">规整服务商</label>
            <CustomSelect
              :model-value="store.apiConfig.polishProvider"
              :options="store.polishProviderOptions.map(o => ({ value: o.type === 'custom' ? store.getCustomOptionValue(o.customProviderId!) : o.providerId, label: o.label, group: o.type, customProviderId: o.customProviderId }))"
              :show-add-custom="true"
              @update:model-value="onPolishProviderChange"
              @add-custom="onCustomAdd"
              @edit-custom="onCustomEdit"
              @delete="onCustomDelete"
            />
          </div>

          <!-- 内置服务商专属：API Key + 模型下拉 -->
          <template v-if="!isCustomPolishProvider">
            <div class="field">
              <label class="label">{{ currentPolishProv.keyLabel }}</label>
              <div class="key-row">
              <input
                :value="store.apiConfig.providerKeys[store.apiConfig.polishProvider] || ''"
                @input="onPolishKeyInput"
                :type="showPolishKey ? 'text' : 'password'"
                class="input"
                :placeholder="currentPolishProv.prefix + '...'"
                @change="onPolishKeyChange"
                @blur="onPolishKeyChange"
              />
              <button class="key-toggle" @click="showPolishKey = !showPolishKey">{{ showPolishKey ? '隐藏' : '显示' }}</button>
            </div>
            <button class="hint-trigger" @click="toggleHelp('polishKey')">这是什么？</button>
            <Transition name="hint-expand">
              <div v-if="showHelp.polishKey" class="hint-box">
                <p class="hint-text">
                  <template v-for="(seg, i) in polishHelpSegments" :key="i">
                    <a v-if="seg.link" href="#" class="hint-link" @click.prevent="onOpenExternal(seg.link.href)">{{ seg.text }}</a>
                    <template v-else>{{ seg.text }}</template>
                  </template>
                </p>
              </div>
            </Transition>
          </div>

          <div class="field">
            <label class="label">规整模型</label>
            <CustomSelect
              :model-value="store.apiConfig.polishModel"
              :options="currentPolishProv.chat.map(m => ({ value: m, label: m }))"
              @update:model-value="onPolishModelChange"
            />
          </div>
          </template>

          <!-- 自定义服务商专属：显示模型下拉 -->
          <template v-else-if="currentCustomPolishProv">
            <div class="field">
              <label class="label">规整模型</label>
              <CustomSelect
                :model-value="store.apiConfig.polishModel"
                :options="store.customPolishModels.map(m => ({ value: m, label: m }))"
                @update:model-value="onPolishModelChange"
              />
            </div>
            <div class="field">
              <label class="label">API Key</label>
              <input class="input input-readonly" value="***" readonly />
              <div class="hint-box">
                <p class="hint-text">自定义服务商的 API Key 已保存。点击上方下拉列表中的服务商名称可编辑。</p>
              </div>
            </div>
          </template>

          <PromptManager ref="promptManagerRef" />

          <div class="field">
            <div class="toggle-row">
              <label class="label" style="margin-bottom:0">日志输出</label>
              <button
                class="toggle"
                :class="{ on: store.apiConfig.loggingEnabled }"
                @click="onToggleLogging(!store.apiConfig.loggingEnabled)"
                :aria-pressed="store.apiConfig.loggingEnabled"
              >
                <span class="toggle-knob" />
              </button>
            </div>
            <div class="hint-box">
              <p class="hint-text">关闭后不再输出运行日志，错误日志始终保留。</p>
            </div>
          </div>

          <div class="field field-footer">
            <div class="footer-actions">
              <button class="key-toggle" @click="openLogDir">查看日志</button>
              <button class="key-toggle" @click="openPromptDir">打开配置目录</button>
            </div>
            <span class="hint-trigger hint-static">遇到问题？请查看日志信息</span>
          </div>
        </div>
      </div>
    </div>
  </Transition>
  <CustomProviderModal
    :visible="customModalVisible"
    :edit-provider="customModalEdit"
    @save="onCustomModalSave"
    @close="onCustomModalClose"
  />
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: opacity 0.25s var(--ease-out);
}

.panel {
  width: min(570px, calc(100vw - 32px));
  height: min(660px, 80vh);
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}

.modal-enter-active .panel,
.modal-leave-active .panel {
  transition: transform 0.25s var(--ease-out), opacity 0.25s var(--ease-out);
}

.modal-enter-from .panel,
.modal-leave-to .panel {
  transform: scale(0.96);
  opacity: 0;
}

.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}

.panel-header {
  height: 52px;
  padding: 0 var(--space-6);
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-hover);
  flex-shrink: 0;
}

.panel-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}

.close-btn {
  width: 28px;
  height: 28px;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-sm);
  transition: all var(--duration-fast) var(--ease-out);
}

.close-btn:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.panel-body {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-6);
}

.save-error {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  margin-bottom: var(--space-4);
  background: var(--danger-muted);
  border: 1px solid var(--danger);
  border-radius: var(--radius-md);
}
.save-error .save-error-text {
  font-size: 12px;
  color: var(--danger);
}

.field {
  margin-bottom: var(--space-6);
}

.label {
  display: block;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: var(--space-2);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.input,
.select {
  width: 100%;
  height: 36px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-elevated);
  color: var(--text-primary);
  font-size: 13px;
  font-family: var(--font-sans);
  outline: none;
  transition: border-color var(--duration-fast) var(--ease-out);
}

.input:focus,
.select:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15);
}

.input-readonly {
  color: var(--text-tertiary);
  cursor: default;
  background: var(--bg-hover);
  border-color: var(--border-subtle);
}

.key-row {
  display: flex;
  gap: var(--space-2);
}

.key-row .input {
  flex: 1;
}

.key-toggle {
  height: 36px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  font-family: var(--font-sans);
  cursor: pointer;
  flex-shrink: 0;
  transition: all var(--duration-fast) var(--ease-out);
}

.key-toggle:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}

.theme-row {
  display: flex;
  gap: var(--space-1);
}

.theme-btn {
  flex: 1;
  height: 32px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: 12px;
  font-family: var(--font-sans);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}

.theme-btn:hover {
  background: var(--bg-hover);
}

.theme-btn.on {
  background: var(--text-primary);
  border-color: var(--text-primary);
  color: var(--bg-base);
}

.textarea {
  width: 100%;
  min-height: 80px;
  padding: var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-elevated);
  color: var(--text-primary);
  font-size: 12px;
  font-family: var(--font-sans);
  line-height: 1.6;
  outline: none;
  resize: vertical;
  transition: border-color var(--duration-fast) var(--ease-out);
}

.textarea:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15);
}

.textarea::placeholder {
  color: var(--text-disabled);
}

.hint-trigger {
  border: none;
  background: none;
  padding: 0;
  margin-top: var(--space-1);
  font-size: 11px;
  color: var(--text-tertiary);
  cursor: pointer;
  font-family: var(--font-sans);
  transition: color var(--duration-fast) var(--ease-out);
}

.hint-trigger:hover {
  color: var(--accent);
}

.hint-static {
  cursor: default;
  margin-top: var(--space-2);
  display: block;
}

.hint-static:hover {
  color: var(--text-tertiary);
}

.toggle-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.toggle {
  width: 36px;
  height: 20px;
  border-radius: 10px;
  border: 1px solid var(--border-default);
  background: var(--border-default);
  cursor: pointer;
  position: relative;
  padding: 0;
  transition: all 0.2s var(--ease-out);
  flex-shrink: 0;
}

.toggle:hover {
  border-color: var(--border-strong);
}

.toggle.on {
  background: var(--accent);
  border-color: var(--accent);
}

.toggle-knob {
  display: block;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--bg-base);
  position: absolute;
  top: 1px;
  left: 1px;
  transition: transform 0.2s var(--ease-out);
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
}

.toggle.on .toggle-knob {
  transform: translateX(16px);
}

.field-footer {
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-default);
}

.field:has(+ .pm) {
  margin-bottom: var(--space-6);
}

.pm {
  margin-bottom: var(--space-6);
}

.pm + .field {
  margin-top: var(--space-2);
}

.footer-actions {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.hint-box {
  margin-top: var(--space-2);
}

.hint-text {
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-tertiary);
  margin: 0;
}

.hint-text a,
.hint-link {
  color: var(--accent);
  text-decoration: none;
  cursor: pointer;
}

.hint-text a:hover,
.hint-link:hover {
  text-decoration: underline;
}

.hint-expand-enter-active,
.hint-expand-leave-active {
  transition: all 0.2s var(--ease-out);
}

.hint-expand-enter-from,
.hint-expand-leave-to {
  opacity: 0;
  max-height: 0;
  margin-top: 0;
  padding-top: 0;
  padding-bottom: 0;
}

.hint-expand-enter-to,
.hint-expand-leave-from {
  opacity: 1;
  max-height: 120px;
}

.section-divider {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-6);
}

.section-divider::before,
.section-divider::after {
  content: '';
  flex: 1;
  height: 1px;
  background: var(--border-default);
}

.section-divider span {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  white-space: nowrap;
}
</style>
