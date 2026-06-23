<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'

const props = defineProps<{
  modelValue: string
  options: { value: string; label: string }[]
  placeholder?: string
}>()

const emit = defineEmits<{ (e: 'update:modelValue', value: string): void }>()

const open = ref(false)
const root = ref<HTMLElement>()

function select(value: string) {
  emit('update:modelValue', value)
  open.value = false
}

function onClickOutside(e: MouseEvent) {
  if (root.value && !root.value.contains(e.target as Node)) {
    open.value = false
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') open.value = false
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
  <div ref="root" class="cs-select" :class="{ open }">
    <button class="cs-trigger" @click="open = !open" type="button">
      <span class="cs-value">{{ options.find(o => o.value === modelValue)?.label || placeholder || '' }}</span>
      <svg class="cs-icon" width="12" height="12" viewBox="0 0 12 12" fill="none">
        <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
      </svg>
    </button>
    <Transition name="cs-dropdown">
      <div v-if="open" class="cs-panel">
        <button
          v-for="opt in options"
          :key="opt.value"
          class="cs-option"
          :class="{ selected: opt.value === modelValue }"
          @click="select(opt.value)"
          type="button"
        >
          {{ opt.label }}
        </button>
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
  transition: border-color var(--duration-fast) var(--ease-out), box-shadow var(--duration-fast) var(--ease-out);
}

.cs-trigger:hover {
  border-color: var(--border-strong);
}

.cs-select.open .cs-trigger {
  border-color: var(--accent);
  box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15);
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
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
  padding: var(--space-1);
  max-height: 200px;
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
