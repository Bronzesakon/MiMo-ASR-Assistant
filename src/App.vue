<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { useSettingsStore } from './stores/settings'
import { useFilesStore } from './stores/files'
import { useUiStore } from './stores/ui'
import { usePromptsStore } from './stores/prompts'
import { useCapabilitiesStore } from './stores/capabilities'
import { setupListeners, teardownListeners } from './utils/events'
import TitleBar from './components/TitleBar.vue'
import Toolbar from './components/Toolbar.vue'
import FileList from './components/FileList.vue'
import Workspace from './components/Workspace.vue'
import StatusBar from './components/StatusBar.vue'
import SettingsDrawer from './components/SettingsDrawer.vue'
import Toast from './components/Toast.vue'

const settings = useSettingsStore()

// P2-02：监听注册全有或全无。failed 时显示重试按钮，重试前先 teardown。
const initializationState = ref<'starting' | 'ready' | 'failed'>('starting')

async function init() {
  initializationState.value = 'starting'
  try {
    // 后端事件监听在应用层显式注册（store 构造无副作用，清单 F-03）。
    // H-01：先 await 全部监听注册完成，再加载配置/提示词，避免一次性恢复提示丢失。
    await setupListeners(useFilesStore(), useUiStore())
    // 配置加载与 splash 最短显示时间并行；提示词文档启动时预加载（F-06.4.4）
    await Promise.all([
      settings.loadConfig().catch(e => console.error('loadConfig failed:', e)),
      usePromptsStore().ensureLoaded().catch(e => console.error('loadPrompts failed:', e)),
      useCapabilitiesStore().ensureLoaded().catch(e => console.error('loadCapabilities failed:', e)),
      new Promise(resolve => setTimeout(resolve, 1200)),
    ])
    initializationState.value = 'ready'
    window.__hideSplash?.()
  } catch (e) {
    console.error('[事件监听] 注册失败:', e)
    teardownListeners()
    initializationState.value = 'failed'
  }
}

function retry() {
  teardownListeners()
  void init()
}

onMounted(() => {
  document.addEventListener('contextmenu', e => e.preventDefault())
  void init()
})

onUnmounted(() => {
  teardownListeners()
})
</script>

<template>
  <div class="app">
    <TitleBar />
    <Toolbar />
    <div class="main">
      <FileList />
      <Workspace />
    </div>
    <StatusBar />
    <SettingsDrawer />
    <Toast />

    <!-- P2-02：监听注册失败时显示重试 -->
    <div v-if="initializationState === 'failed'" class="init-failed">
      <div class="init-failed-box">
        <p>事件监听初始化失败，功能可能不可用。</p>
        <button @click="retry">重试</button>
      </div>
    </div>
  </div>
</template>

<style>
*,
*::before,
*::after {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

:root {
  /* Typography scale */
  --font-sans: 'SF Pro Display', 'Geist Sans', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  --font-mono: 'SF Mono', 'Geist Mono', 'JetBrains Mono', 'Cascadia Code', monospace;

  /* Spacing scale */
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 20px;
  --space-6: 24px;
  --space-7: 28px;
  --space-8: 32px;

  /* Radius scale */
  --radius-xs: 4px;
  --radius-sm: 6px;
  --radius-md: 8px;
  --radius-lg: 12px;

  /* Transition */
  --ease-out: cubic-bezier(0.16, 1, 0.3, 1);
  --duration-fast: 120ms;
  --duration-normal: 200ms;
}

/* Dark theme */
:root,
[data-theme="dark"] {
  --bg-base: #111111;
  --bg-surface: #191919;
  --bg-elevated: #1f1f1f;
  --bg-overlay: #252525;
  --bg-hover: #2a2a2a;
  --bg-active: #333333;
  --text-primary: #ededec;
  --text-secondary: #a1a1a0;
  --text-tertiary: #6f6f6e;
  --text-disabled: #4a4a49;
  --border-default: #272726;
  --border-subtle: #1f1f1e;
  --border-strong: #3a3a38;
  --accent: #6e9eff;
  --accent-hover: #85aeff;
  --accent-muted: rgba(110, 158, 255, 0.12);
  --success: #5cb85c;
  --success-muted: rgba(92, 184, 92, 0.12);
  --warning: #d4a84b;
  --warning-muted: rgba(212, 168, 75, 0.12);
  --danger: #d45b5b;
  --danger-muted: rgba(212, 91, 91, 0.12);
}

/* Light theme */
[data-theme="light"] {
  --bg-base: #f8f8f7;
  --bg-surface: #ffffff;
  --bg-elevated: #f5f5f4;
  --bg-overlay: #eeeeed;
  --bg-hover: #e8e8e7;
  --bg-active: #dededd;
  --text-primary: #1a1a19;
  --text-secondary: #6b6b6a;
  --text-tertiary: #9c9c9b;
  --text-disabled: #c4c4c3;
  --border-default: #e4e4e3;
  --border-subtle: #ebebea;
  --border-strong: #d0d0cf;
  --accent: #2563eb;
  --accent-hover: #3b82f6;
  --accent-muted: rgba(37, 99, 235, 0.08);
  --success: #16a34a;
  --success-muted: rgba(22, 163, 74, 0.08);
  --warning: #ca8a04;
  --warning-muted: rgba(202, 138, 4, 0.08);
  --danger: #dc2626;
  --danger-muted: rgba(220, 38, 38, 0.08);
}

html, body {
  height: 100%;
  overflow: hidden;
}

body {
  font-family: var(--font-sans);
  background-color: var(--bg-base);
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
}

.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}

.main {
  display: flex;
  flex: 1;
  overflow: hidden;
}

/* Scrollbar — hidden by default */
::-webkit-scrollbar {
  width: 4px;
  height: 4px;
}

::-webkit-scrollbar-track {
  background: transparent;
}

::-webkit-scrollbar-thumb {
  background: transparent;
  border-radius: 2px;
  transition: background 0.2s;
}

*:hover::-webkit-scrollbar-thumb {
  background: var(--border-default);
}

::-webkit-scrollbar-thumb:hover {
  background: var(--border-strong);
}

/* Selection */
::selection {
  background: var(--accent-muted);
  color: var(--text-primary);
}

/* App entrance animation */
@keyframes app-fade-in {
  from { opacity: 0; transform: translateY(4px); }
  to { opacity: 1; transform: translateY(0); }
}

.app {
  animation: app-fade-in 0.35s var(--ease-out) both;
}

/* P2-02：初始化失败重试层 */
.init-failed {
  position: fixed;
  inset: 0;
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  background: color-mix(in srgb, var(--bg-base) 80%, transparent);
  backdrop-filter: blur(4px);
}
.init-failed-box {
  padding: var(--space-6);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: var(--bg-surface);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-4);
  color: var(--text-primary);
  font-size: 13px;
}
.init-failed-box button {
  height: 30px;
  padding: 0 var(--space-5);
  border: 1px solid var(--accent);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--accent);
  font-size: 12px;
  font-weight: 500;
  font-family: var(--font-sans);
  cursor: pointer;
}
.init-failed-box button:hover { background: var(--accent-muted); }
</style>
