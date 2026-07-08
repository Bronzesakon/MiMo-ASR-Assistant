<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'

export interface SelectOption {
  value: string
  label: string
  group?: 'builtin' | 'custom'
  customProviderId?: string
}

const props = defineProps<{
  modelValue: string
  options: SelectOption[]
  placeholder?: string
  disabled?: boolean
  showAddCustom?: boolean
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', value: string): void
  (e: 'delete', customProviderId: string): void
  (e: 'addCustom'): void
  (e: 'editCustom', customProviderId: string): void
}>()

const open = ref(false)
const root = ref<HTMLElement>()

const builtinOptions = () => props.options.filter(o => o.group !== 'custom')
const customOptions = () => props.options.filter(o => o.group === 'custom')
const hasCustom = () => customOptions().length > 0 || props.showAddCustom

function select(value: string) {
  if (props.disabled) return
  emit('update:modelValue', value)
  open.value = false
}

function deleteOption(e: Event, customProviderId: string) {
  e.stopPropagation()
  emit('delete', customProviderId)
}

function editCustom(e: Event, customProviderId: string) {
  e.stopPropagation()
  emit('editCustom', customProviderId)
  open.value = false
}

function addCustom(e: Event) {
  e.stopPropagation()
  open.value = false
  emit('addCustom')
}

function onClickOutside(e: MouseEvent) {
  if (root.value && !root.value.contains(e.target as Node)) {
    open.value = false
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') open.value = false
}

function toggleOpen() {
  if (props.disabled) return
  open.value = !open.value
}

onMounted(() => {
  document.addEventListener('mousedown', onClickOutside)
  document.addEventListener('keydown', onKeydown)
})
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onClickOutside)
  document.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div ref="root" class="cs-select" :class="{ open, disabled }">
    <button class="cs-trigger" @click="toggleOpen" type="button" :disabled="disabled">
      <span class="cs-value">{{ options.find(o => o.value === modelValue)?.label || placeholder || '' }}</span>
      <svg class="cs-icon" width="12" height="12" viewBox="0 0 12 12" fill="none">
        <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
    </button>
    <Transition name="cs-dropdown">
      <div v-if="open" class="cs-panel">
        <!-- 内置选项 -->
        <button
          v-for="opt in builtinOptions()"
          :key="opt.value"
          class="cs-option"
          :class="{ selected: opt.value === modelValue }"
          @click="select(opt.value)"
          type="button"
        >
          {{ opt.label }}
        </button>

        <!-- 分隔线 -->
        <template v-if="hasCustom()">
          <div class="cs-divider" />

          <!-- 自定义选项 -->
          <div v-for="opt in customOptions()" :key="opt.value" class="cs-option-row">
            <button
              class="cs-option cs-option-custom"
              :class="{ selected: opt.value === modelValue }"
              @click="select(opt.value)"
              type="button"
            >
              {{ opt.label }}
            </button>
            <button
              class="cs-edit-btn"
              @click="editCustom($event, opt.customProviderId || '')"
              title="编辑"
              type="button"
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
                <path d="M7.5 1.25L8.75 2.5L3 8.25H1.75V7L7.5 1.25Z" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </button>
            <button
              class="cs-delete-btn"
              @click="deleteOption($event, opt.customProviderId || '')"
              title="删除"
              type="button"
            >
              <svg width="10" height="10" viewBox="0 0 10 10"><path d="M2.5 2.5l5 5M7.5 2.5l-5 5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
            </button>
          </div>

          <!-- 添加入口 -->
          <button
            v-if="showAddCustom"
            class="cs-option cs-add-btn"
            @click="addCustom"
            type="button"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
              <path d="M5 1v8M1 5h8" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
            </svg>
            添加自定义服务商
          </button>
        </template>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.cs-select {
  position: relative;
  width: 100%;
}

.cs-trigger {
  width: 100%;
  height: 36px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-elevated);
  color: var(--text-primary);
  font-size: 13px;
  font-family: var(--font-sans);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: space-between;
  outline: none;
  transition: border-color var(--duration-fast) var(--ease-out), box-shadow var(--duration-fast) var(--ease-out), background var(--duration-fast) var(--ease-out);
}

.cs-trigger:hover {
  border-color: var(--border-strong);
}

.cs-select.open .cs-trigger {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px rgba(110, 158, 255, 0.1);
  background: var(--bg-base);
}

.cs-select.disabled .cs-trigger {
  opacity: 0.35;
  background: var(--bg-hover);
  cursor: not-allowed;
  pointer-events: none;
}

.cs-value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  text-align: left;
}

.cs-icon {
  flex-shrink: 0;
  color: var(--text-tertiary);
  margin-left: var(--space-2);
  transition: transform var(--duration-fast) var(--ease-out);
}

.cs-select.open .cs-icon {
  transform: rotate(180deg);
}

.cs-panel {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  z-index: 100;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.25), 0 0 0 1px rgba(0,0,0,0.05);
  max-height: 260px;
  overflow-y: auto;
}

.cs-option {
  width: 100%;
  height: 32px;
  padding: 0 var(--space-3);
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
  font-family: var(--font-sans);
  cursor: pointer;
  display: flex;
  align-items: center;
  text-align: left;
  transition: background var(--duration-fast) var(--ease-out);
}

.cs-option:hover {
  background: var(--bg-hover);
}

.cs-option.selected {
  color: var(--accent);
  font-weight: 500;
}

.cs-divider {
  height: 1px;
  background: var(--border-default);
  margin: var(--space-1) var(--space-2);
}

.cs-option-row {
  display: flex;
  align-items: center;
  position: relative;
}

.cs-option-custom {
  flex: 1;
  min-width: 0;
}

.cs-delete-btn {
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
  opacity: 0;
  transition: all var(--duration-fast) var(--ease-out);
  flex-shrink: 0;
  margin-right: var(--space-1);
}

.cs-option-row:hover .cs-delete-btn,
.cs-option-row:hover .cs-edit-btn {
  opacity: 1;
}

.cs-delete-btn:hover,
.cs-edit-btn:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.cs-edit-btn {
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
  opacity: 0;
  transition: all var(--duration-fast) var(--ease-out);
  flex-shrink: 0;
  margin-right: var(--space-1);
}

.cs-add-btn {
  color: var(--accent);
  font-size: 12px;
  gap: var(--space-1);
  margin-top: var(--space-1);
  border-top: 1px solid var(--border-subtle);
  padding-top: var(--space-2);
}

.cs-add-btn svg {
  opacity: 0.7;
}

.cs-add-btn:hover {
  background: var(--accent-muted);
}

/* 下拉动画 */
.cs-dropdown-enter-active,
.cs-dropdown-leave-active {
  transition: opacity 0.15s var(--ease-out), transform 0.15s var(--ease-out);
}

.cs-dropdown-enter-from,
.cs-dropdown-leave-to {
  opacity: 0;
  transform: translateY(-4px) scale(0.97);
}
</style>
