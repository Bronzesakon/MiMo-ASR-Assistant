<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import { CONTENT_MAX_CHARS } from '../stores/prompts'
import type { PromptEntry } from '../types'

const props = defineProps<{
  entry: PromptEntry | null
  expanded: boolean
  dragDisabled: boolean
  isDragging: boolean
  dropPosition: 'above' | 'below' | null
  canMoveUp?: boolean
  canMoveDown?: boolean
  draftTitle: string
  draftContent: string
  errorMsg: string
  saving: boolean
}>()

const emit = defineEmits<{
  (e: 'edit'): void
  (e: 'delete'): void
  (e: 'save'): void
  (e: 'cancel'): void
  (e: 'restore-default'): void
  (e: 'update:draftTitle', value: string): void
  (e: 'update:draftContent', value: string): void
  (e: 'move-up'): void
  (e: 'move-down'): void
}>()

const titleRef = ref<HTMLInputElement | null>(null)
const contentRef = ref<HTMLTextAreaElement | null>(null)

const contentCount = () => Array.from(props.draftContent).length

watch(() => props.expanded, async (v) => {
  if (!v) return
  await nextTick()
  const target = props.entry?.is_default ? contentRef.value : titleRef.value
  target?.focus()
})

</script>

<template>
  <div
    class="prow"
    :class="{ dragging: isDragging, 'drop-above': dropPosition === 'above', 'drop-below': dropPosition === 'below' }"
    data-testid="prompt-row"
  >
    <!-- 收起横条：标题 → 编辑 → 删除（默认项不渲染）→ 最右拖拽柄（F-06.5.3） -->
    <div v-if="entry && !expanded" class="prow-bar">
      <span class="prow-title" :title="entry.title">{{ entry.title }}</span>
      <button
        class="prow-btn"
        title="编辑"
        aria-label="编辑提示词"
        data-testid="prompt-edit"
        @click="emit('edit')"
      >
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
          <path d="M8.2 1.6l2.2 2.2L4 10.2l-2.6.6.6-2.6 6.2-6.6z" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
      <button
        v-if="!entry.is_default"
        class="prow-btn prow-btn-danger"
        title="删除"
        aria-label="删除提示词"
        data-testid="prompt-delete"
        @click="emit('delete')"
      >
        <svg width="10" height="10" viewBox="0 0 10 10">
          <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
        </svg>
      </button>
      <button class="prow-btn prow-btn-narrow" :disabled="dragDisabled || !canMoveUp" data-testid="prompt-move-up" title="上移" aria-label="上移提示词" @click="emit('move-up')">↑</button>
      <button class="prow-btn prow-btn-narrow" :disabled="dragDisabled || !canMoveDown" draggable="false" data-testid="prompt-grip" title="下移" aria-label="下移提示词" @click="emit('move-down')">↓</button>
    </div>

    <!-- 展开编辑器：与横条同一条目，边框/背景连续（F-06.8 第三阶段） -->
    <div v-else-if="expanded || !entry" class="prow-editor" data-testid="prompt-editor">
      <div v-if="entry && entry.is_default" class="prow-fixed-title">默认</div>
      <input
        v-else
        ref="titleRef"
        class="prow-input"
        :value="draftTitle"
        placeholder="标题（不超过 24 字符）"
        data-testid="prompt-title-input"
        @input="emit('update:draftTitle', ($event.target as HTMLInputElement).value)"
      />
      <textarea
        ref="contentRef"
        class="prow-textarea"
        :value="draftContent"
        placeholder="提示词正文（不超过 4000 字符）"
        data-testid="prompt-content-input"
        @input="emit('update:draftContent', ($event.target as HTMLTextAreaElement).value)"
      ></textarea>
      <div class="prow-meta">
        <span v-if="errorMsg" class="prow-error">{{ errorMsg }}</span>
        <span v-else />
        <span class="prow-count" :class="{ over: contentCount() > CONTENT_MAX_CHARS }">
          {{ contentCount() }} / {{ CONTENT_MAX_CHARS }}
        </span>
      </div>
      <div class="prow-footer">
        <button
          v-if="entry && entry.is_default"
          class="prow-action"
          :disabled="saving"
          data-testid="prompt-restore-default"
          @click="emit('restore-default')"
        >恢复默认</button>
        <span v-else />
        <div class="prow-actions">
          <button class="prow-action" :disabled="saving" data-testid="prompt-cancel" @click="emit('cancel')">取消</button>
          <button
            class="prow-action prow-action-primary"
            :disabled="saving || contentCount() > CONTENT_MAX_CHARS"
            data-testid="prompt-save"
            @click="emit('save')"
          >{{ saving ? '保存中…' : '保存' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.prow {
  position: relative;
}
.prow.dragging {
  opacity: 0.5;
}
/* drop target：1-2px accent 插入线，不改变条目高度（F-06.8 第四阶段） */
.prow.drop-above::before,
.prow.drop-below::after {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  height: 2px;
  background: var(--accent);
  border-radius: 1px;
  pointer-events: none;
}
.prow.drop-above::before { top: -3px; }
.prow.drop-below::after { bottom: -3px; }

.prow-bar {
  min-height: 40px;
  display: flex;
  align-items: center;
  gap: var(--space-1);
  padding: 0 var(--space-2);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.prow-title {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.prow-btn {
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
.prow-btn:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}
.prow-btn:active {
  transform: scale(0.97);
}
.prow-btn:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.prow-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.prow-btn-danger:hover {
  color: var(--danger);
  background: var(--danger-muted);
}
/* 上移/下移按钮缩窄为默认宽度的五分之三（28 * 3/5 ≈ 17px） */
.prow-btn-narrow {
  width: 17px;
}
.prow-editor {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-3);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
}

.prow-fixed-title {
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
}

.prow-input {
  width: 100%;
  height: 36px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-base);
  color: var(--text-primary);
  font-size: 13px;
  font-family: var(--font-sans);
  outline: none;
  transition: border-color var(--duration-fast) var(--ease-out);
}
.prow-input:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15);
}
.prow-input::placeholder {
  color: var(--text-disabled);
}

.prow-textarea {
  width: 100%;
  max-width: 100%;
  min-height: 160px;
  padding: var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-base);
  color: var(--text-primary);
  font-size: 12px;
  font-family: var(--font-sans);
  line-height: 1.6;
  outline: none;
  resize: vertical;
  transition: border-color var(--duration-fast) var(--ease-out);
}
.prow-textarea:focus {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15);
}
.prow-textarea::placeholder {
  color: var(--text-disabled);
}

.prow-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}
.prow-error {
  font-size: 11px;
  color: var(--danger);
}
.prow-count {
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--text-tertiary);
  flex-shrink: 0;
}
.prow-count.over {
  color: var(--danger);
}

.prow-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}
.prow-actions {
  display: flex;
  gap: var(--space-2);
}
.prow-action {
  height: 36px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: 12px;
  font-family: var(--font-sans);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.prow-action:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}
.prow-action:active {
  transform: scale(0.97);
}
.prow-action:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.prow-action:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.prow-action-primary {
  color: var(--accent);
  border-color: var(--accent);
}
.prow-action-primary:hover {
  background: var(--accent-muted);
  border-color: var(--accent-hover);
}
</style>
