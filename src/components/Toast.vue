<script setup lang="ts">
import { useAppStore } from '../stores/app'
const store = useAppStore()
</script>

<template>
  <Transition name="toast">
    <div v-if="store.showToast" class="toast" :class="'toast-' + store.toastType">
      {{ store.toastMessage }}
    </div>
  </Transition>
</template>

<style scoped>
.toast {
  position: fixed;
  top: 56px;
  left: 50%;
  transform: translateX(-50%);
  padding: var(--space-2) var(--space-4);
  border-radius: var(--radius-md);
  font-size: 12px;
  font-weight: 500;
  z-index: 9999;
  pointer-events: none;
  border: 1px solid;
  backdrop-filter: blur(8px);
}

.toast-info {
  background: var(--bg-elevated);
  color: var(--text-primary);
  border-color: var(--border-default);
}

.toast-error {
  background: var(--danger-muted);
  color: var(--danger);
  border-color: var(--danger);
}

.toast-success {
  background: var(--success-muted);
  color: var(--success);
  border-color: var(--success);
}

.toast-enter-active,
.toast-leave-active {
  transition: all 0.25s var(--ease-out);
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(-6px);
}
</style>
