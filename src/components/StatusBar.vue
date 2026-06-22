<script setup lang="ts">
import { useAppStore } from '../stores/app'
const store = useAppStore()

function fmtSpeed(b: number) {
  if (!b) return '--'
  return b < 1048576 ? `${(b/1024).toFixed(1)} KB/s` : `${(b/1048576).toFixed(1)} MB/s`
}
</script>

<template>
  <div class="status">
    <span class="status-model">{{ store.statusInfo.modelName }}</span>
    <span class="status-sep" />
    <span class="status-label">上传</span>
    <span class="status-val">{{ fmtSpeed(store.statusInfo.uploadSpeed) }}</span>
    <span class="status-sep" />
    <span class="status-label">输出</span>
    <span class="status-val">{{ store.statusInfo.outputSpeed }} 字/s</span>
    <span class="status-sep" />
    <span class="status-label">Token</span>
    <span class="status-val">{{ store.statusInfo.tokenCount.toLocaleString() }}</span>
    <span class="status-spacer" />
    <span class="status-val">{{ store.completedCount }}/{{ store.files.length }}</span>
  </div>
</template>

<style scoped>
.status {
  height: 26px;
  background: var(--bg-surface);
  border-top: 1px solid var(--border-default);
  display: flex;
  align-items: center;
  padding: 0 var(--space-3);
  gap: var(--space-2);
  flex-shrink: 0;
}

.status-model {
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--text-tertiary);
}

.status-label {
  font-size: 10px;
  color: var(--text-disabled);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.status-val {
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--text-tertiary);
}

.status-sep {
  width: 1px;
  height: 10px;
  background: var(--border-default);
}

.status-spacer {
  flex: 1;
}
</style>
