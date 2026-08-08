<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'

export interface ConfirmAction {
  value: string
  label: string
  variant?: 'default' | 'danger' | 'primary'
}

const props = defineProps<{
  visible: boolean
  title: string
  message: string
  actions: ConfirmAction[]
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
        class="cd-overlay"
        tabindex="-1"
        @click.self="emit('close')"
        @keydown="onKeydown"
      >
        <div class="cd-dialog" data-testid="confirm-dialog" @click.stop>
          <div class="cd-dialog-title">{{ title }}</div>
          <div class="cd-dialog-message">{{ message }}</div>
          <div class="cd-dialog-actions">
            <button
              v-for="act in actions"
              :key="act.value"
              class="cd-dialog-action"
              :class="act.variant === 'danger' ? 'cd-danger' : act.variant === 'primary' ? 'cd-primary' : ''"
              @click.stop="emit('select', act.value)"
            >
              {{ act.label }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.cd-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  z-index: 1100;
  display: flex;
  align-items: center;
  justify-content: center;
  outline: none;
}
.cd-dialog {
  width: 280px;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}
.cd-dialog-title {
  height: 40px;
  padding: 0 var(--space-4);
  display: flex;
  align-items: center;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  border-bottom: 1px solid var(--border-default);
}
.cd-dialog-message {
  padding: var(--space-3) var(--space-4);
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-all;
}
.cd-dialog-actions {
  padding: var(--space-1);
}
.cd-dialog-action {
  display: flex;
  align-items: center;
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
.cd-dialog-action:hover {
  background: var(--bg-hover);
}
.cd-dialog-action:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.cd-dialog-action.cd-danger {
  color: var(--danger);
}
.cd-dialog-action.cd-danger:hover {
  background: var(--danger-muted);
}
.cd-dialog-action.cd-primary {
  color: var(--accent);
  font-weight: 500;
}

.modal-enter-active .cd-dialog,
.modal-leave-active .cd-dialog {
  transition: transform 0.2s var(--ease-out), opacity 0.2s var(--ease-out);
}
.modal-enter-from .cd-dialog,
.modal-leave-to .cd-dialog {
  transform: scale(0.96);
  opacity: 0;
}
.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}
</style>
