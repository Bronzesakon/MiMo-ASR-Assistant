<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { usePromptsStore, TITLE_MAX_CHARS, CONTENT_MAX_CHARS } from '../stores/prompts'
import { useSettingsStore } from '../stores/settings'
import { useUiStore } from '../stores/ui'
import PromptRow from './PromptRow.vue'
import ConfirmDialog from './ConfirmDialog.vue'
import type { PromptDraft, PromptEntry } from '../types'

const store = usePromptsStore()
const settings = useSettingsStore()
const ui = useUiStore()

const draft = ref<PromptDraft | null>(null)
const original = ref<{ title: string; content: string } | null>(null)
const saveError = ref('')
const deleteTarget = ref<PromptEntry | null>(null)
const closeConfirm = ref(false)

type Pending =
  | { type: 'edit'; entry: PromptEntry }
  | { type: 'new' }
  | { type: 'close' }
const pending = ref<Pending | null>(null)

onMounted(() => {
  store.ensureLoaded()
})

const dragDisabled = computed(() => draft.value !== null || store.writing || store.loading || !store.doc)

const addDisabled = computed(() =>
  !store.doc || store.loading || store.writing || store.atLimit || draft.value?.id === null
)

const addTitle = computed(() =>
  store.atLimit ? '最多保存 8 条提示词，请先删除不需要的条目' : '新增提示词'
)

const isDirty = computed(() => {
  if (!draft.value || !original.value) return false
  return draft.value.title !== original.value.title || draft.value.content !== original.value.content
})

function openDraft(entry: PromptEntry) {
  draft.value = { id: entry.id, title: entry.title, content: entry.content }
  original.value = { title: entry.title, content: entry.content }
  saveError.value = ''
}

function requestCreate() {
  if (addDisabled.value) return
  if (isDirty.value) {
    pending.value = { type: 'new' }
    closeConfirm.value = true
    return
  }
  draft.value = { id: null, title: '', content: '' }
  original.value = { title: '', content: '' }
  saveError.value = ''
}

function requestEdit(entry: PromptEntry) {
  if (draft.value?.id === entry.id) return
  if (isDirty.value) {
    pending.value = { type: 'edit', entry }
    closeConfirm.value = true
    return
  }
  openDraft(entry)
}

function cancelDraft() {
  draft.value = null
  original.value = null
  saveError.value = ''
}

function draftIsDefault(): boolean {
  const id = draft.value?.id
  if (!id) return false
  return store.prompts.some(p => p.id === id && p.is_default)
}

async function saveDraft() {
  if (!draft.value || store.writing) return
  const id = draft.value.id
  const isDefault = draftIsDefault()
  const titleT = draft.value.title.trim()
  const contentT = draft.value.content.trim()
  if (!isDefault) {
    const titleLen = Array.from(titleT).length
    if (titleLen === 0) { saveError.value = '标题不能为空'; return }
    if (titleLen > TITLE_MAX_CHARS) { saveError.value = `标题不能超过 ${TITLE_MAX_CHARS} 个字符`; return }
    if (store.prompts.some(p => p.id !== id && p.title === titleT)) {
      saveError.value = '已存在相同标题的提示词'
      return
    }
  }
  const contentLen = Array.from(contentT).length
  if (contentLen === 0) { saveError.value = '正文不能为空'; return }
  if (contentLen > CONTENT_MAX_CHARS) { saveError.value = `正文不能超过 ${CONTENT_MAX_CHARS} 个字符`; return }
  try {
    if (id === null) {
      await store.createPrompt(titleT, contentT)
    } else {
      await store.updatePrompt(id, isDefault ? null : titleT, contentT)
    }
    draft.value = null
    original.value = null
    saveError.value = ''
  } catch (e) {
    saveError.value = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
    ui.showToastMessage('提示词保存失败', 'error')
  }
}

async function restoreDefault() {
  if (!draft.value) return
  try {
    const content = await store.builtinDefaultContent()
    draft.value = { ...draft.value, content }
    saveError.value = ''
  } catch (e) {
    console.error('[提示词] 获取默认正文失败:', e)
  }
}

/** SettingsDrawer 关闭前调用；有未保存变更时弹三选确认并阻止关闭 */
async function requestClose(): Promise<boolean> {
  if (isDirty.value) {
    pending.value = { type: 'close' }
    closeConfirm.value = true
    return false
  }
  return true
}

async function onCloseConfirm(value: string) {
  closeConfirm.value = false
  const p = pending.value
  pending.value = null
  if (value === 'stay') return
  if (value === 'save') {
    await saveDraft()
    if (draft.value !== null) return
  } else {
    draft.value = null
    original.value = null
    saveError.value = ''
  }
  if (!p) return
  if (p.type === 'edit') {
    openDraft(p.entry)
  } else if (p.type === 'new') {
    draft.value = { id: null, title: '', content: '' }
    original.value = { title: '', content: '' }
  } else {
    ui.isSettingsOpen = false
    settings.flush().catch(() => {})
  }
}

function requestDelete(entry: PromptEntry) {
  deleteTarget.value = entry
}

async function onDeleteConfirm(value: string) {
  const target = deleteTarget.value
  deleteTarget.value = null
  if (value !== 'confirm' || !target) return
  try {
    await store.deletePrompt(target.id)
  } catch (e) {
    ui.showToastMessage(`删除失败: ${String(e).replace(/^Error invoking plugin /, '').slice(0, 100)}`, 'error')
  }
}

async function movePrompt(index: number, direction: -1 | 1) {
  if (dragDisabled.value) return
  const ids = store.prompts.map(p => p.id)
  const target = index + direction
  if (target < 0 || target >= ids.length) return
  ;[ids[index], ids[target]] = [ids[target], ids[index]]
  try {
    await store.reorderPrompts(ids)
  } catch (e) {
    console.error('[提示词] 排序保存失败:', e)
    ui.showToastMessage('排序保存失败，已保持原顺序', 'error')
  }
}

defineExpose({ requestClose })
</script>

<template>
  <div class="pm" data-testid="prompt-manager">
    <div class="pm-header">
      <span class="pm-label">AI 规整提示词</span>
      <button
        class="pm-add"
        :disabled="addDisabled"
        :title="addTitle"
        aria-label="新增提示词"
        data-testid="prompt-add"
        @click="requestCreate"
      >
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
          <path d="M6 2v8M2 6h8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
        </svg>
      </button>
    </div>

    <div v-if="store.loading" class="pm-status">正在加载提示词…</div>
    <div v-else-if="!store.doc" class="pm-status pm-status-error">
      <span>无法加载提示词</span>
      <button class="pm-retry" @click="store.ensureLoaded(true)">重试</button>
    </div>
    <div v-else class="pm-list">
      <PromptRow
        v-if="draft && draft.id === null"
        :entry="null"
        :expanded="true"
        :drag-disabled="true"
        :is-dragging="false"
        :drop-position="null"
        :draft-title="draft.title"
        :draft-content="draft.content"
        :error-msg="saveError"
        :saving="store.writing"
        @save="saveDraft"
        @cancel="cancelDraft"
        @update:draft-title="(v: string) => { if (draft) draft.title = v }"
        @update:draft-content="(v: string) => { if (draft) draft.content = v }"
      />
      <div
        v-for="(p, idx) in store.prompts"
        :key="p.id"
      >
        <PromptRow
          :entry="p"
          :expanded="draft?.id === p.id"
          :drag-disabled="dragDisabled"
          :is-dragging="false"
          :drop-position="null"
          :can-move-up="idx > 0"
          :can-move-down="idx < store.prompts.length - 1"
          :draft-title="draft?.id === p.id ? draft!.title : ''"
          :draft-content="draft?.id === p.id ? draft!.content : ''"
          :error-msg="draft?.id === p.id ? saveError : ''"
          :saving="store.writing"
          data-testid="prompt-row-wrap"
          @edit="requestEdit(p)"
          @delete="requestDelete(p)"
          @save="saveDraft"
          @cancel="cancelDraft"
          @restore-default="restoreDefault"
          @update:draft-title="(v: string) => { if (draft) draft.title = v }"
          @update:draft-content="(v: string) => { if (draft) draft.content = v }"
          @move-up="movePrompt(idx, -1)"
          @move-down="movePrompt(idx, 1)"
        />
      </div>
      <div v-if="store.atLimit" class="pm-limit">最多保存 8 条提示词，请先删除不需要的条目</div>
    </div>

    <ConfirmDialog
      :visible="deleteTarget !== null"
      title="删除提示词"
      :message="`确定删除「${deleteTarget?.title ?? ''}」吗？删除后不可恢复。`"
      :actions="[
        { value: 'cancel', label: '取消' },
        { value: 'confirm', label: '删除', variant: 'danger' },
      ]"
      @select="onDeleteConfirm"
      @close="deleteTarget = null"
    />
    <ConfirmDialog
      :visible="closeConfirm"
      title="未保存的修改"
      message="当前提示词有未保存的修改，如何处理？"
      :actions="[
        { value: 'save', label: '保存并继续', variant: 'primary' },
        { value: 'discard', label: '放弃修改' },
        { value: 'stay', label: '继续编辑' },
      ]"
      @select="onCloseConfirm"
      @close="onCloseConfirm('stay')"
    />
  </div>
</template>

<style scoped>
.pm {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.pm-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--space-2);
}
.pm-label {
  display: block;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.pm-add {
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
  flex-shrink: 0;
  transition: all var(--duration-fast) var(--ease-out);
}
.pm-add:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}
.pm-add:active {
  transform: scale(0.97);
}
.pm-add:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.pm-add:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.pm-list {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}
.pm-status {
  padding: var(--space-3);
  font-size: 12px;
  color: var(--text-tertiary);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}
.pm-status-error {
  color: var(--danger);
}
.pm-retry {
  border: none;
  background: none;
  padding: 0;
  font-size: 12px;
  color: var(--accent);
  cursor: pointer;
  font-family: var(--font-sans);
}
.pm-retry:hover {
  color: var(--accent-hover);
}
.pm-limit {
  font-size: 11px;
  line-height: 1.6;
  color: var(--text-tertiary);
}
</style>
