<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'

export interface SingleChoiceOption {
  value: string
  label: string
}

const props = defineProps<{
  visible: boolean
  title: string
  options: SingleChoiceOption[]
  activeValue?: string | null
  highlight: 'check' | 'background'
}>()

const emit = defineEmits<{
  (e: 'select', value: string): void
  (e: 'close'): void
}>()

const overlayRef = ref<HTMLDivElement | null>(null)

watch(() => props.visible, (v) => {
  if (v) nextTick(() => overlayRef.value?.focus())
})

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('close')
}
</script>

<template>
  <Teleport to="body">
    <Transition name="modal">
      <div
        v-if="visible"
        ref="overlayRef"
        class="sc-overlay"
        tabindex="-1"
        @click.self="emit('close')"
        @keydown="onKeydown"
      >
        <div class="sc-dialog" data-testid="single-choice-dialog" @click.stop>
          <div class="sc-dialog-title">{{ props.title }}</div>
          <div class="sc-dialog-options">
            <button
              v-for="opt in props.options"
              :key="opt.value"
              class="sc-dialog-option"
              :class="{
                active: props.highlight === 'check' && opt.value === props.activeValue,
                recent: props.highlight === 'background' && opt.value === props.activeValue,
              }"
              :title="opt.label"
              @click.stop="emit('select', opt.value)"
            >
              <span class="sc-dialog-label">{{ opt.label }}</span>
              <svg
                v-if="props.highlight === 'check' && opt.value === props.activeValue"
                class="sc-dialog-check" width="14" height="14" viewBox="0 0 14 14" fill="none"
              >
                <path d="M3 7.5l3 3 5-6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.sc-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  outline: none;
}
.sc-dialog {
  width: 240px;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}
.sc-dialog-title {
  height: 40px;
  padding: 0 var(--space-4);
  display: flex;
  align-items: center;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  border-bottom: 1px solid var(--border-default);
}
.sc-dialog-options {
  padding: var(--space-1);
}
.sc-dialog-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  height: 36px;
  padding: 0 var(--space-3);
  border: none;
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: 13px;
  font-family: var(--font-sans);
  cursor: pointer;
  text-align: left;
  transition: background var(--duration-fast) var(--ease-out);
}
.sc-dialog-option:hover {
  background: var(--bg-hover);
}
.sc-dialog-option:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.sc-dialog-option.active {
  color: var(--accent);
  font-weight: 500;
}
/* 最近项仅背景高亮：不显示勾选、图标或标签（F-06.6.3） */
.sc-dialog-option.recent {
  background: var(--bg-hover);
}
.sc-dialog-label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sc-dialog-check {
  color: var(--accent);
  flex-shrink: 0;
}

.modal-enter-active .sc-dialog,
.modal-leave-active .sc-dialog {
  transition: transform 0.2s var(--ease-out), opacity 0.2s var(--ease-out);
}
.modal-enter-from .sc-dialog,
.modal-leave-to .sc-dialog {
  transform: scale(0.96);
  opacity: 0;
}
.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}
</style>
