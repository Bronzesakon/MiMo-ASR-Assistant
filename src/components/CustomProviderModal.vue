<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useAppStore } from '../stores/app'
import type { CustomProvider } from '../types'

const props = defineProps<{
  visible: boolean
  editProvider?: CustomProvider | null
}>()

const emit = defineEmits<{
  (e: 'save', provider: CustomProvider): void
  (e: 'close'): void
}>()

const store = useAppStore()

const name = ref('')
const baseUrl = ref('')
const apiKey = ref('')
const showKey = ref(false)
const models = ref<string[]>([])
const selectedModels = ref<Set<string>>(new Set())
const loading = ref(false)
const fetched = ref(false)

watch(() => props.visible, (v) => {
  if (v) {
    if (props.editProvider) {
      name.value = props.editProvider.name
      baseUrl.value = props.editProvider.baseUrl
      apiKey.value = props.editProvider.apiKey
      models.value = [...props.editProvider.models]
      selectedModels.value = new Set(props.editProvider.models)
      fetched.value = true
    } else {
      name.value = ''
      baseUrl.value = ''
      apiKey.value = ''
      models.value = []
      selectedModels.value.clear()
      fetched.value = false
    }
    showKey.value = false
  }
})

const allSelected = computed(() => models.value.length > 0 && models.value.every(m => selectedModels.value.has(m)))
const selectedCount = computed(() => selectedModels.value.size)

function toggleModel(model: string) {
  if (selectedModels.value.has(model)) {
    selectedModels.value.delete(model)
  } else {
    selectedModels.value.add(model)
  }
  selectedModels.value = new Set(selectedModels.value)
}

function selectAll() {
  selectedModels.value = new Set(models.value)
}

function deselectAll() {
  selectedModels.value = new Set()
}

async function fetchModels() {
  if (!baseUrl.value.trim() || !apiKey.value.trim()) {
    store.showToastMessage('请填写 Base URL 和 API Key', 'error')
    return
  }
  loading.value = true
  try {
    const result = await invoke<string[]>('fetch_custom_models', {
      baseUrl: baseUrl.value.trim(),
      apiKey: apiKey.value.trim(),
    })
    models.value = result
    // 保留已选中的模型
    const newSelected = new Set<string>()
    for (const m of result) {
      if (selectedModels.value.has(m)) newSelected.add(m)
    }
    selectedModels.value = newSelected
    fetched.value = true
    if (result.length === 0) {
      store.showToastMessage('未获取到任何模型', 'info')
    }
  } catch (e) {
    const msg = String(e).replace(/^Error invoking plugin /, '')
    if (msg.includes('401') || msg.includes('403')) {
      store.showToastMessage('认证失败，请检查 API Key', 'error')
    } else if (msg.includes('404') || msg.includes('405')) {
      store.showToastMessage('端点不存在，请检查 Base URL', 'error')
    } else if (msg.includes('timeout') || msg.includes('超时')) {
      store.showToastMessage('请求超时，请检查网络连接', 'error')
    } else {
      store.showToastMessage(`获取模型失败: ${msg.slice(0, 100)}`, 'error')
    }
  } finally {
    loading.value = false
  }
}

function save() {
  if (!name.value.trim()) {
    store.showToastMessage('请填写服务商名称', 'error')
    return
  }
  if (!baseUrl.value.trim()) {
    store.showToastMessage('请填写 Base URL', 'error')
    return
  }
  if (!apiKey.value.trim()) {
    store.showToastMessage('请填写 API Key', 'error')
    return
  }
  if (selectedModels.value.size === 0) {
    store.showToastMessage('请至少选择一个模型', 'error')
    return
  }

  const provider: CustomProvider = {
    id: props.editProvider?.id || `custom_${Date.now()}`,
    name: name.value.trim(),
    baseUrl: baseUrl.value.trim(),
    apiKey: apiKey.value.trim(),
    models: Array.from(selectedModels.value),
  }

  emit('save', provider)
}

function close() {
  emit('close')
}

function onOverlayKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
}
</script>

<template>
  <Transition name="modal">
    <div v-if="visible" class="overlay" @click.self="close" @keydown="onOverlayKeydown" tabindex="-1">
      <div class="panel">
        <div class="panel-header">
          <span class="panel-title">{{ editProvider ? '编辑自定义服务商' : '添加自定义服务商' }}</span>
          <button class="close-btn" @click="close">
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
        </div>

        <div class="panel-body">
          <div class="field">
            <label class="label">服务商名称</label>
            <input v-model="name" class="input" placeholder="例如: OpenRouter" />
          </div>

          <div class="field">
            <label class="label">Base URL</label>
            <input v-model="baseUrl" class="input" placeholder="https://openrouter.ai/api/v1" />
          </div>

          <div class="field">
            <label class="label">API Key</label>
            <div class="key-row">
              <input
                v-model="apiKey"
                :type="showKey ? 'text' : 'password'"
                class="input"
                placeholder="sk-..."
              />
              <button class="key-toggle" @click="showKey = !showKey">{{ showKey ? '隐藏' : '显示' }}</button>
            </div>
          </div>

          <div class="field">
            <button class="fetch-btn" @click="fetchModels" :disabled="loading">
              <span v-if="loading" class="spinner" />
              {{ loading ? '获取中...' : '获取模型列表' }}
            </button>
          </div>

          <template v-if="fetched">
            <div class="field model-header">
              <label class="label">选择模型</label>
              <div class="model-actions">
                <button class="mini-btn" @click="allSelected ? deselectAll() : selectAll()">
                  {{ allSelected ? '取消全选' : '全选' }}
                </button>
                <span class="selected-count">已选 {{ selectedCount }} 个</span>
              </div>
            </div>

            <div class="model-list">
              <div v-if="models.length === 0" class="model-empty">未获取到模型</div>
              <label
                v-for="m in models"
                :key="m"
                class="model-item"
                :class="{ checked: selectedModels.has(m) }"
              >
                <input
                  type="checkbox"
                  :checked="selectedModels.has(m)"
                  @change="toggleModel(m)"
                  class="model-checkbox"
                />
                <span class="model-name">{{ m }}</span>
              </label>
            </div>
          </template>
        </div>

        <div class="panel-footer">
          <button class="btn-cancel" @click="close">取消</button>
          <button class="btn-save" @click="save">保存</button>
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
  z-index: 1100;
  display: flex;
  align-items: center;
  justify-content: center;
}

.panel {
  width: 500px;
  max-height: 80vh;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  display: flex;
  flex-direction: column;
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
  margin-bottom: var(--space-5);
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

.input {
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
  transition: border-color var(--duration-fast) var(--ease-out), box-shadow var(--duration-fast) var(--ease-out), background var(--duration-fast) var(--ease-out);
}
.input:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px rgba(110, 158, 255, 0.1);
  background: var(--bg-base);
}
.input::placeholder {
  color: var(--text-disabled);
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

.fetch-btn {
  width: 100%;
  height: 36px;
  border: 1px solid var(--accent);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--accent);
  font-size: 13px;
  font-weight: 500;
  font-family: var(--font-sans);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  transition: all var(--duration-fast) var(--ease-out);
}
.fetch-btn:hover {
  background: var(--accent-muted);
}
.fetch-btn:active {
  transform: scale(0.98);
}
.fetch-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
  transform: none;
}

.spinner {
  width: 14px;
  height: 14px;
  border: 2px solid var(--accent-muted);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}

.model-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.model-header .label {
  margin-bottom: 0;
}
.model-actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}
.mini-btn {
  height: 24px;
  padding: 0 var(--space-2);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  font-size: 11px;
  font-family: var(--font-sans);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.mini-btn:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}
.selected-count {
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--text-disabled);
}

.model-list {
  max-height: 240px;
  overflow-y: auto;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  margin-top: var(--space-2);
}
.model-empty {
  padding: var(--space-4);
  text-align: center;
  font-size: 12px;
  color: var(--text-disabled);
}
.model-item {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out);
}
.model-item:hover {
  background: var(--bg-hover);
}
.model-item + .model-item {
  border-top: 1px solid var(--border-subtle);
}
.model-checkbox {
  appearance: none;
  width: 16px;
  height: 16px;
  border: 1.5px solid var(--border-strong);
  border-radius: var(--radius-xs);
  background: var(--bg-elevated);
  position: relative;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
  flex-shrink: 0;
}
.model-checkbox:checked {
  background: var(--accent);
  border-color: var(--accent);
}
.model-checkbox:checked::after {
  content: '';
  position: absolute;
  left: 4px;
  top: 1px;
  width: 5px;
  height: 9px;
  border: solid white;
  border-width: 0 1.5px 1.5px 0;
  transform: rotate(45deg);
}
.model-name {
  font-size: 12px;
  color: var(--text-primary);
  font-family: var(--font-mono);
  word-break: break-all;
}

.panel-footer {
  padding: var(--space-4) var(--space-6);
  border-top: 1px solid var(--border-default);
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  flex-shrink: 0;
}
.btn-cancel,
.btn-save {
  height: 32px;
  padding: 0 var(--space-4);
  border-radius: var(--radius-md);
  font-size: 13px;
  font-weight: 500;
  font-family: var(--font-sans);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.btn-cancel {
  border: 1px solid var(--border-default);
  background: transparent;
  color: var(--text-secondary);
}
.btn-cancel:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}
.btn-save {
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #ffffff;
}
.btn-save:hover {
  background: var(--accent-hover);
  border-color: var(--accent-hover);
}
</style>
