<script setup lang="ts">
import { useAppStore } from '../stores/app'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'

const store = useAppStore()

function openSettings() { store.isSettingsOpen = true }

function fmtDur(s: number) {
  if (!s || s <= 0) return '--:--'
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = Math.floor(s % 60)
  return h > 0
    ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`
    : `${m}:${String(sec).padStart(2, '0')}`
}

async function importAudio() {
  if (!store.apiConfig.apiKey?.trim()) {
    store.showToastMessage('请先填写 API Key', 'error')
    store.isSettingsOpen = true
    return
  }
  try {
    const selected = await open({
      multiple: true,
      filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'm4a', 'flac', 'aac', 'ogg'] }]
    })
    if (!selected) return
    for (const path of (Array.isArray(selected) ? selected : [selected])) {
      const name = path.split(/[/\\]/).pop() || path
      const id = Date.now().toString(36) + Math.random().toString(36).slice(2, 8)
      let duration = 0
      try { duration = await invoke<number>('get_audio_duration', { filePath: path }) } catch {}
      store.addFile({
        id, name, path, duration, size: 0,
        status: 'waiting', progress: 0,
        transcription: '', polished: '',
        transcriptionWordCount: 0, polishedWordCount: 0,
        polishStage: 'idle',
      })
      store.selectFile(id)
    }
  } catch (e) {
    console.error('Import failed:', e)
  }
}
</script>

<template>
  <div class="toolbar">
    <div class="toolbar-left">
      <button class="btn" @click="importAudio">
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><path d="M7 2v10M3.5 5.5L7 2l3.5 3.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>
        导入
      </button>
    </div>
    <div class="toolbar-center">
      <span v-if="store.selectedFile" class="file-badge">
        {{ store.selectedFile.name }}
        <span v-if="store.selectedFile.duration > 0" class="dur">{{ fmtDur(store.selectedFile.duration) }}</span>
      </span>
    </div>
    <div class="toolbar-right">
      <button class="btn" @click="openSettings" title="设置">
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none"><circle cx="7" cy="7" r="2" stroke="currentColor" stroke-width="1.2"/><path d="M7 1.5v1.5M7 11v1.5M1.5 7H3M11 7h1.5M3.05 3.05l1.06 1.06M9.89 9.89l1.06 1.06M10.95 3.05l-1.06 1.06M4.11 9.89l-1.06 1.06" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
        设置
      </button>
    </div>
  </div>
</template>

<style scoped>
.toolbar {
  height: 48px;
  background: var(--bg-surface);
  border-bottom: 1px solid var(--border-default);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 var(--space-4);
  flex-shrink: 0;
  gap: var(--space-2);
}

.toolbar-left, .toolbar-right {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  min-width: 80px;
}

.toolbar-right {
  justify-content: flex-end;
}

.toolbar-center {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.file-badge {
  font-size: 12px;
  color: var(--text-secondary);
  display: inline-flex;
  align-items: center;
  gap: var(--space-2);
  line-height: 1;
}

.dur {
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--text-disabled);
  padding: 2px var(--space-1);
  background: var(--bg-elevated);
  border-radius: var(--radius-sm);
  line-height: 1;
  display: inline-flex;
  align-items: center;
}

.btn {
  height: 30px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-primary);
  font-size: 12px;
  font-weight: 500;
  font-family: var(--font-sans);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: var(--space-1);
  transition: all 0.15s var(--ease-out);
  white-space: nowrap;
}

.btn:hover { background: var(--bg-hover); border-color: var(--border-strong); }
.btn:active { transform: scale(0.97); background: var(--bg-active); }
</style>
