<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'

const appWindow = getCurrentWindow()

function minimize() { appWindow.minimize() }
function toggleMaximize() { appWindow.toggleMaximize() }
function close() { invoke('hide_main_window').catch(() => {}) }

async function onMouseDown(e: MouseEvent) {
  // 只在标题栏空白区域拖拽，按钮不触发
  if ((e.target as HTMLElement).closest('[data-no-drag]')) return
  try {
    await appWindow.startDragging()
  } catch {}
}
</script>

<template>
  <div class="title-bar" @mousedown="onMouseDown">
    <div class="title-bar-left">
      <span class="title-bar-text">MiMo ASR Assistant</span>
    </div>
    <div class="title-bar-buttons" data-no-drag>
      <button class="title-btn" @click="minimize" title="最小化">
        <svg width="10" height="1" viewBox="0 0 10 1"><line x1="0" y1="0.5" x2="10" y2="0.5" stroke="currentColor" stroke-width="1"/></svg>
      </button>
      <button class="title-btn" @click="toggleMaximize" title="最大化">
        <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" stroke="currentColor" stroke-width="1" fill="none" rx="1"/></svg>
      </button>
      <button class="title-btn title-btn-close" @click="close" title="关闭">
        <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5" stroke="currentColor" stroke-width="1" stroke-linecap="round"/></svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.title-bar {
  height: 38px;
  background: var(--bg-surface);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--space-3);
  flex-shrink: 0;
  user-select: none;
  -webkit-user-select: none;
  border-bottom: 1px solid var(--border-default);
  cursor: default;
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  pointer-events: none;
}

.title-bar-text {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
  letter-spacing: 0.03em;
  margin-left: var(--space-2);
}

.title-bar-buttons {
  display: flex;
  align-items: center;
}

.title-btn {
  width: 36px;
  height: 38px;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
}

.title-btn:hover {
  background: var(--bg-hover);
  color: var(--text-secondary);
}

.title-btn:active {
  background: var(--bg-active);
}

.title-btn-close:hover {
  background: var(--danger);
  color: #ffffff;
}
</style>
