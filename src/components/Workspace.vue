<script setup lang="ts">
import { useAppStore } from '../stores/app'
import { ref, watch, nextTick, computed } from 'vue'
import { markdownToHtml } from '../utils/markdown'

const store = useAppStore()
const textRef = ref<HTMLElement | null>(null)
const polishRef = ref<HTMLElement | null>(null)
const autoScroll = ref(true)
const copiedOriginal = ref(false)
const copiedPolished = ref(false)

function renderMd(text: string): string {
  return markdownToHtml(text || '')
}

// 双栏模式：文件有规整内容或正在规整中 → 显示双栏（不依赖 polishEnabledFileId）
const showDualPanel = computed(() => {
  const file = store.selectedFile
  if (!file) return false
  const hasPolishState = (file.polished?.length ?? 0) > 0 || file.status === 'polishing'
  return hasPolishState && (file.transcription?.length ?? 0) > 0
})

const hasOriginal = computed(() =>
  (store.selectedFile?.transcription?.length ?? 0) > 0
)
const hasPolished = computed(() =>
  (store.selectedFile?.polished?.length ?? 0) > 0
)

watch(() => store.selectedFile?.transcription, () => {
  if (autoScroll.value) nextTick(() => textRef.value && (textRef.value.scrollTop = textRef.value.scrollHeight))
})
watch(() => store.selectedFile?.polished, () => {
  if (autoScroll.value) nextTick(() => polishRef.value && (polishRef.value.scrollTop = polishRef.value.scrollHeight))
})

function onScroll() {
  const el = textRef.value
  if (el) autoScroll.value = Math.abs(el.scrollHeight - el.scrollTop - el.clientHeight) < 60
}

function goBottom() {
  autoScroll.value = true
  textRef.value && (textRef.value.scrollTop = textRef.value.scrollHeight)
  polishRef.value && (polishRef.value.scrollTop = polishRef.value.scrollHeight)
}

async function copyText(type: 'original' | 'polished') {
  const text = type === 'original' ? store.selectedFile?.transcription : store.selectedFile?.polished
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
    if (type === 'original') {
      copiedOriginal.value = true
      setTimeout(() => { copiedOriginal.value = false }, 1500)
    } else {
      copiedPolished.value = true
      setTimeout(() => { copiedPolished.value = false }, 1500)
    }
  } catch (e) {
    console.error('Copy failed:', e)
  }
}
</script>

<template>
  <div class="workspace">
    <!-- 空状态 -->
    <div v-if="!store.selectedFile" class="ws-empty">
      <div class="ws-empty-content">
        <svg width="32" height="32" viewBox="0 0 32 32" fill="none" class="ws-empty-icon">
          <rect x="3" y="3" width="26" height="26" rx="4" stroke="currentColor" stroke-width="1.5"/>
          <path d="M10 12h12M10 16h8M10 20h10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
        </svg>
        <span class="ws-empty-text">转写结果将在此显示</span>
      </div>
    </div>

    <!-- 单栏 -->
    <div v-else-if="!showDualPanel" class="ws-single">
      <div class="ws-header">
        <span class="ws-title">转写结果</span>
        <span v-if="store.selectedFile.transcriptionWordCount > 0" class="ws-count">{{ store.selectedFile.transcriptionWordCount }} 字</span>
        <div class="ws-header-right">
          <button v-if="hasOriginal" class="ws-btn" :class="{ copied: copiedOriginal }" @click="copyText('original')">
            {{ copiedOriginal ? '已复制' : '复制' }}
          </button>
          <button v-if="!autoScroll && hasOriginal" class="ws-btn" @click="goBottom">
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M5 1v8M2 6l3 3 3-3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>
          </button>
        </div>
      </div>
      <div ref="textRef" class="ws-body" @scroll="onScroll">
        <div v-if="store.selectedFile.transcription" class="ws-text">{{ store.selectedFile.transcription }}</div>
        <div v-else class="ws-hint">
          <template v-if="store.selectedFile.status === 'waiting'">点击左侧"转写"按钮开始</template>
          <template v-else-if="store.selectedFile.status === 'processing'">音频处理中...</template>
          <template v-else-if="store.selectedFile.status === 'transcribing'">正在转写...</template>
          <template v-else-if="store.selectedFile.status === 'failed'">转写失败</template>
          <template v-else>暂无内容</template>
        </div>
      </div>
    </div>

    <!-- 双栏 -->
    <div v-else class="ws-dual">
      <div class="ws-panel">
        <div class="ws-header">
          <span class="ws-title">转写原文</span>
          <span v-if="store.selectedFile.transcriptionWordCount > 0" class="ws-count">{{ store.selectedFile.transcriptionWordCount }} 字</span>
          <div class="ws-header-right">
            <button v-if="hasOriginal" class="ws-btn" :class="{ copied: copiedOriginal }" @click="copyText('original')">
              {{ copiedOriginal ? '已复制' : '复制' }}
            </button>
            <button v-if="!autoScroll && hasOriginal" class="ws-btn" @click="goBottom">
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M5 1v8M2 6l3 3 3-3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </button>
          </div>
        </div>
        <div ref="textRef" class="ws-body" @scroll="onScroll">
          <div v-if="store.selectedFile.transcription" class="ws-text md-content" v-html="renderMd(store.selectedFile.transcription)"></div>
          <div v-else class="ws-hint">暂无内容</div>
        </div>
      </div>
      <div class="ws-divider" />
      <div class="ws-panel">
        <div class="ws-header">
          <span class="ws-title">AI 规整</span>
          <span v-if="store.selectedFile.polishedWordCount > 0" class="ws-count">{{ store.selectedFile.polishedWordCount }} 字</span>
          <div class="ws-header-right">
            <button v-if="hasPolished" class="ws-btn" :class="{ copied: copiedPolished }" @click="copyText('polished')">
              {{ copiedPolished ? '已复制' : '复制' }}
            </button>
          </div>
        </div>
        <div ref="polishRef" class="ws-body">
          <div v-if="store.selectedFile.polished" class="ws-text md-content" v-html="renderMd(store.selectedFile.polished)"></div>
          <div v-else class="ws-hint">
            <template v-if="store.selectedFile.status === 'polishing'">{{ (store.selectedFile as any).polishStage === 'sending' ? '发送中…' : (store.selectedFile as any).polishStage === 'receiving' ? '接收中…' : '规整中…' }}</template>
            <template v-else>等待转写完成</template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.workspace {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--bg-base);
}

.ws-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* App entrance animation */
@keyframes ws-fade-in {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}

.ws-empty-content {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--space-3);
  animation: ws-fade-in 0.5s var(--ease-out) 0.1s both;
}

.ws-empty-icon { color: var(--text-disabled); opacity: 0.5; }
.ws-empty-text { font-size: 13px; color: var(--text-disabled); }

.ws-header {
  height: 36px;
  padding: 0 var(--space-4);
  display: flex;
  align-items: center;
  gap: var(--space-3);
  border-bottom: 1px solid var(--border-default);
  background: var(--bg-surface);
  flex-shrink: 0;
}

.ws-title {
  font-size: 11px;
  font-weight: 500;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.ws-count {
  font-size: 10px;
  font-family: var(--font-mono);
  color: var(--text-disabled);
}

.ws-header-right {
  display: flex;
  align-items: center;
  gap: var(--space-1);
  margin-left: auto;
}

.ws-btn {
  height: 24px;
  padding: 0 var(--space-2);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-tertiary);
  font-size: 11px;
  font-family: var(--font-sans);
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: var(--space-1);
  transition: all var(--duration-fast) var(--ease-out);
}

.ws-btn:hover { background: var(--bg-hover); border-color: var(--border-strong); }
.ws-btn.copied { color: var(--success); border-color: var(--success); }

.ws-body {
  flex: 1;
  overflow-y: auto;
  padding: var(--space-5) var(--space-6);
}

.ws-text {
  font-size: 14px;
  line-height: 1.75;
  color: var(--text-primary);
  word-break: break-word;
}

.ws-hint {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  font-size: 13px;
  color: var(--text-disabled);
}

.ws-single { flex: 1; display: flex; flex-direction: column; overflow: hidden; }
.ws-dual { flex: 1; display: flex; overflow: hidden; }
.ws-panel { flex: 1; display: flex; flex-direction: column; overflow: hidden; }
.ws-divider { width: 1px; background: var(--border-subtle); flex-shrink: 0; }

/* Markdown 样式 */
:deep(.md-content) h1,
:deep(.md-content) h2,
:deep(.md-content) h3 {
  font-weight: 600;
  color: var(--text-primary);
  margin: 1em 0 0.5em;
  line-height: 1.4;
}
:deep(.md-content) h1 { font-size: 1.3em; }
:deep(.md-content) h2 { font-size: 1.15em; }
:deep(.md-content) h3 { font-size: 1.05em; }

:deep(.md-content) p {
  margin: 0.4em 0;
}

:deep(.md-content) strong {
  font-weight: 600;
  color: var(--text-primary);
}

:deep(.md-content) em {
  font-style: italic;
}

:deep(.md-content) code {
  font-family: var(--font-mono);
  font-size: 0.9em;
  background: var(--bg-elevated);
  padding: 1px 5px;
  border-radius: 3px;
  color: var(--accent);
}

:deep(.md-content) pre {
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  padding: var(--space-3);
  margin: 0.6em 0;
  overflow-x: auto;
}

:deep(.md-content) pre code {
  background: none;
  padding: 0;
  color: var(--text-primary);
  font-size: 0.85em;
}

:deep(.md-content) ul,
:deep(.md-content) ol {
  padding-left: 1.5em;
  margin: 0.4em 0;
}

:deep(.md-content) li {
  margin: 0.15em 0;
}

:deep(.md-content) blockquote {
  border-left: 3px solid var(--accent);
  padding-left: var(--space-3);
  margin: 0.6em 0;
  color: var(--text-secondary);
}

:deep(.md-content) hr {
  border: none;
  border-top: 1px solid var(--border-default);
  margin: 1em 0;
}

:deep(.md-content) a {
  color: var(--accent);
  text-decoration: none;
}
:deep(.md-content) a:hover {
  text-decoration: underline;
}

:deep(.md-content) del {
  color: var(--text-tertiary);
}
</style>
