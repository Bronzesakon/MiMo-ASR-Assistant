<script setup lang="ts">
import { useAppStore } from '../stores/app'
import { invoke } from '@tauri-apps/api/core'
import { ref, computed, reactive } from 'vue'
import CustomSelect from './CustomSelect.vue'

const store = useAppStore()
const showKey = ref(false)
const showHelp = reactive<Record<string, boolean>>({ key: false })

function toggleHelp(id: string) {
  showHelp[id] = !showHelp[id]
}

interface HelpLink { label: string; href: string }

function getHelpLinks(): { text: string; links: HelpLink[] } {
  const id = store.apiConfig.provider
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

const providers = [
  { id: 'mimo-api', name: 'MiMo API', url: 'https://api.xiaomimimo.com/v1', prefix: 'sk-', keyLabel: 'MiMo API Key', asr: ['mimo-v2.5-asr'], chat: ['mimo-v2.5', 'mimo-v2.5-pro'] },
  { id: 'mimo-token-plan', name: 'MiMo Token Plan', url: 'https://token-plan-cn.xiaomimimo.com/v1', prefix: 'tp-', keyLabel: 'MiMo Token Plan API Key', asr: ['mimo-v2.5-asr'], chat: ['mimo-v2.5', 'mimo-v2.5-pro'] },
  { id: 'deepseek', name: 'DeepSeek', url: 'https://api.deepseek.com', prefix: 'sk-', keyLabel: 'DeepSeek API Key', asr: [], chat: ['deepseek-v4-pro', 'deepseek-v4-flash'] },
]

const prov = computed(() => providers.find(p => p.id === store.apiConfig.provider) || providers[0])

// 当前服务商的 API Key（从 providerKeys 中读取）
const currentKey = computed({
  get: () => store.apiConfig.providerKeys[store.apiConfig.provider] || '',
  set: (val: string) => {
    store.apiConfig.providerKeys[store.apiConfig.provider] = val
    // 同步到 apiKey（保持向后兼容）
    store.apiConfig.apiKey = val
  }
})

function getHelpContent(data: { text: string; links: HelpLink[] }): string {
  let result = data.text
  data.links.forEach((link, i) => {
    result = result.replace(`{${i}}`, `<a href="${link.href}" target="_blank" rel="noopener noreferrer">${link.label}</a>`)
  })
  return result
}

function close() { store.isSettingsOpen = false; store.saveConfig() }

function onProvider() {
  store.apiConfig.baseUrl = prov.value.url
  // 切换服务商时，从 providerKeys 读取对应的 key
  store.apiConfig.apiKey = store.apiConfig.providerKeys[prov.value.id] || ''
  if (!prov.value.asr.includes(store.apiConfig.transcriptionModel)) store.apiConfig.transcriptionModel = prov.value.asr[0] || ''
  if (!prov.value.chat.includes(store.apiConfig.polishModel)) store.apiConfig.polishModel = prov.value.chat[0]
  store.saveConfig()
}

function onKeyChange() {
  // 保存当前 key 到 providerKeys
  store.apiConfig.providerKeys[store.apiConfig.provider] = currentKey.value
  store.saveConfig()
}

function onOverlayKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}

function openLogDir() {
  invoke('open_log_dir').catch(() => {})
}
</script>

<template>
  <Transition name="modal">
    <div class="overlay" @click.self="close" @keydown="onOverlayKeydown" tabindex="-1">
      <div class="panel">
        <div class="panel-header">
          <span class="panel-title">设置</span>
          <button class="close-btn" @click="close">
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
        </div>

        <div class="panel-body">
          <div class="field">
            <label class="label">主题</label>
            <div class="theme-row">
              <button v-for="t in ['system','light','dark'] as const" :key="t" class="theme-btn" :class="{ on: store.theme === t }" @click="store.setTheme(t)">
                {{ t === 'system' ? '跟随系统' : t === 'light' ? '亮色' : '暗色' }}
              </button>
            </div>
          </div>

          <div class="field">
            <label class="label">服务商</label>
            <CustomSelect
              :model-value="store.apiConfig.provider"
              :options="providers.map(p => ({ value: p.id, label: p.name }))"
              @update:model-value="(v: string) => { store.apiConfig.provider = v; onProvider() }"
            />
          </div>

          <div class="field">
            <label class="label">{{ prov.keyLabel }}</label>
            <div class="key-row">
              <input v-model="currentKey" :type="showKey ? 'text' : 'password'" class="input" :placeholder="prov.prefix + '...'" @change="onKeyChange" />
              <button class="key-toggle" @click="showKey = !showKey">{{ showKey ? '隐藏' : '显示' }}</button>
            </div>
            <button class="hint-trigger" @click="toggleHelp('key')">这是什么？</button>
            <Transition name="hint-expand">
              <div v-if="showHelp.key" class="hint-box">
                <p class="hint-text" v-html="getHelpContent(getHelpLinks())"></p>
              </div>
            </Transition>
          </div>

          <div class="field">
            <label class="label">规整模型</label>
            <CustomSelect
              :model-value="store.apiConfig.polishModel"
              :options="prov.chat.map(m => ({ value: m, label: m }))"
              @update:model-value="(v: string) => { store.apiConfig.polishModel = v; store.saveConfig() }"
            />
          </div>

          <div class="field">
            <label class="label">AI 规整提示词</label>
            <textarea v-model="store.apiConfig.polishPrompt" class="textarea" rows="4" placeholder="自定义 AI 规整的提示词..." @change="store.saveConfig()"></textarea>
            <div class="hint-box">
              <p class="hint-text">留空使用默认提示词。修改后自动保存。</p>
            </div>
          </div>

          <div class="field field-footer">
            <button class="key-toggle" @click="openLogDir">查看日志</button>
            <span class="hint-trigger hint-static">遇到问题？请查看日志信息</span>
          </div>
        </div>
      </div>
    </div>
  </Transition>
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
}

.panel {
  width: 570px;
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

.field-footer {
  padding-top: var(--space-4);
  border-top: 1px solid var(--border-default);
}

.hint-box {
  margin-top: var(--space-2);
  padding: var(--space-3);
  background: var(--bg-hover);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  overflow: hidden;
}

.hint-text {
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-tertiary);
  margin: 0;
}

.hint-text :deep(a) {
  color: var(--accent);
  text-decoration: none;
}

.hint-text :deep(a:hover) {
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
</style>
