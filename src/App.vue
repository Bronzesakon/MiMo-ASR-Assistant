<script setup lang="ts">
import { onMounted } from 'vue'
import { useAppStore } from './stores/app'
import TitleBar from './components/TitleBar.vue'
import Toolbar from './components/Toolbar.vue'
import FileList from './components/FileList.vue'
import Workspace from './components/Workspace.vue'
import StatusBar from './components/StatusBar.vue'
import SettingsDrawer from './components/SettingsDrawer.vue'
import Toast from './components/Toast.vue'

const store = useAppStore()

onMounted(async () => {
  document.addEventListener('contextmenu', e => e.preventDefault())
  const start = Date.now()
  // 配置加载与 splash 最短显示时间并行
  await Promise.all([
    store.loadConfig().catch(e => console.error('loadConfig failed:', e)),
    new Promise(resolve => setTimeout(resolve, 1200)),
  ])
  ;(window as any).__hideSplash?.()
  console.log(`[启动] splash 显示 ${Date.now() - start}ms`)
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
    <SettingsDrawer v-if="store.isSettingsOpen" />
    <Toast />
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
  --space-8: 32px;

  /* Radius scale */
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
</style>
