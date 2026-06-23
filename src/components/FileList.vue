<script setup lang="ts">
import { useAppStore } from '../stores/app'
import { invoke } from '@tauri-apps/api/core'
import { save } from '@tauri-apps/plugin-dialog'
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { listen } from '@tauri-apps/api/event'

const store = useAppStore()
const query = ref('')
const showLangMenu = ref<string | null>(null) // fileId or null

const LANGUAGES = [
  { code: 'auto', label: '自动检测' },
  { code: 'zh', label: '中文' },
  { code: 'en', label: '英文' },
  { code: 'ja', label: '日文' },
]

function getSavedLang(): string {
  return localStorage.getItem('mimo-language') || 'auto'
}
function saveLang(lang: string) {
  localStorage.setItem('mimo-language', lang)
}

const files = computed(() => {
  if (!query.value) return store.files
  const q = query.value.toLowerCase()
  return store.files.filter(f => f.name.toLowerCase().includes(q))
})

const statusMap: Record<string, { label: string; cls: string }> = {
  waiting: { label: '等待转写', cls: 's-waiting' },
  processing: { label: '准备中', cls: 's-active' },
  transcribing: { label: '转写中', cls: 's-active' },
  polishing: { label: '规整中', cls: 's-active' },
  completed: { label: '已完成', cls: 's-done' },
  failed: { label: '失败', cls: 's-error' },
}

const processingStages: Record<string, string> = {}
const unlistenFns: Array<() => void> = []

onMounted(async () => {
  const fn = await listen<string>('process-status', (event) => {
    try {
      const data = JSON.parse(event.payload)
      processingStages[data.file_id] = data.stage || '处理中'
    } catch {}
  })
  unlistenFns.push(fn)
})

onUnmounted(() => unlistenFns.forEach(fn => fn()))

function fmtDur(s: number) {
  if (!s) return '--:--'
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${String(sec).padStart(2, '0')}`
}

function select(id: string) { store.selectFile(id); closeAllDropdowns() }
function remove(id: string, e: Event) { e.stopPropagation(); store.removeFile(id) }
function closeAllDropdowns() {
  document.querySelectorAll('.export-wrap.open').forEach(el => el.classList.remove('open'))
  showLangMenu.value = null
}

// 显示语言选择
function showLanguageSelector(fileId: string) {
  const file = store.files.find(f => f.id === fileId)
  if (!file || file.status === 'transcribing' || file.status === 'processing' || file.status === 'polishing') return
  showLangMenu.value = fileId
}

// 确认语言后开始转写
function confirmLanguage(fileId: string, language: string) {
  showLangMenu.value = null
  saveLang(language)
  transcribeFile(fileId, language)
}

async function transcribeFile(fileId: string, language: string) {
  const file = store.files.find(f => f.id === fileId)
  if (!file || file.status === 'transcribing' || file.status === 'processing') return

  console.log(`[转写] 开始: ${file.name} (${fileId}), 语言=${language}`)

  store.updateFile(fileId, {
    status: 'processing', progress: 0,
    transcription: '', polished: '',
    transcriptionWordCount: 0, polishedWordCount: 0,
    polishStage: 'idle', error: undefined,
  })
  store.isTranscribing = true

  const state = store.ensureFileState(fileId)
  state.transcribeStartTime = Date.now()
  state.outputCharCount = 0
  state.uploadSpeedSum = 0
  state.uploadSpeedCount = 0
  state.totalSlices = 0
  state.currentSlice = 0

  try {
    console.log(`[转写] 开始处理并转写: ${file.name}`)
    store.updateFile(fileId, { status: 'transcribing' })

    await invoke('process_and_transcribe', {
      fileId,
      filePath: file.path,
      apiParams: {
        base_url: store.apiConfig.baseUrl,
        api_key: store.apiConfig.apiKey,
        model: store.apiConfig.transcriptionModel,
        language,
      },
    })
    console.log(`[转写] 完成: ${file.name}`)
  } catch (e) {
    console.error(`[转写] 失败: ${file.name} (${fileId})`, e)
    const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
    store.updateFile(fileId, { status: 'failed', error: errMsg })
    store.showToastMessage(`${file?.name || '文件'} 转写失败`, 'error')
  }

  store.isTranscribing = store.hasActiveFiles
}

const POLISH_PROMPT = `给你以下语音转写结果，理解内容，根据相关领域的正确内容和相关术语、知识，联系上下文勘误部分转文字错误，去掉口语化表达的语气词，要求只对每句话做修饰，每句话都要输出，不简略，根据原文的内容，智能切分长段落`

async function polishFile(fileId: string) {
  const file = store.files.find(f => f.id === fileId)
  if (!file || !file.transcription || file.status === 'polishing') return

  const polishPrompt = store.apiConfig.polishPrompt || POLISH_PROMPT

  console.log(`[规整] 开始: ${file.name} (${fileId}), 文本长度=${file.transcription.length}`)

  store.polishEnabledFileId = fileId
  store.selectFile(fileId)
  store.updateFile(fileId, {
    polished: '', polishedWordCount: 0,
    status: 'polishing', polishStage: 'sending', error: undefined,
  })

  // 重置 polish 状态
  const state = store.ensureFileState(fileId)
  state.polishChunkCount = 0
  state.polishFirstChunk = false

  try {
    await invoke('start_chat', {
      fileId,
      systemPrompt: polishPrompt,
      userText: file.transcription,
      audioDataUrl: null,
      apiParams: {
        base_url: store.apiConfig.baseUrl,
        api_key: store.apiConfig.apiKey,
        model: store.apiConfig.polishModel,
        language: 'auto',
      },
      responseFormat: null,
    })
    console.log(`[规整] 完成: ${file.name}`)
  } catch (e) {
    console.error(`[规整] 失败: ${file.name} (${fileId})`, e)
    const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
    store.updateFile(fileId, { status: 'completed', polishStage: 'failed', error: errMsg })
    store.showToastMessage(`${file?.name || '文件'} 规整失败`, 'error')
  }
}

async function exportFile(fileId: string, type: 'original' | 'polished') {
  const file = store.files.find(f => f.id === fileId)
  if (!file) return
  const content = type === 'original' ? file.transcription : file.polished
  if (!content) {
    store.showToastMessage(`${file.name} ${type === 'original' ? '暂无原文' : '暂无规整结果'}`, 'error')
    return
  }
  const suffix = type === 'original' ? '_原文' : '_规整'
  const defaultName = `${file.name.replace(/\.[^.]+$/, '')}${suffix}.txt`
  try {
    const filePath = await save({
      defaultPath: defaultName,
      filters: [{ name: 'Text', extensions: ['txt'] }]
    })
    if (!filePath) return
    await invoke('save_file', { content, path: filePath })
    store.showToastMessage(`已保存: ${filePath}`, 'success')
  } catch (e) {
    console.error(`[导出] 失败:`, e)
    store.showToastMessage(`${file.name} 导出失败`, 'error')
  }
}

function toggleExport(event: Event, fileId: string) {
  const file = store.files.find(f => f.id === fileId)
  if (!file || file.status !== 'completed' || (!file.transcription && !file.polished)) return
  const wrap = event.currentTarget as HTMLElement
  const isOpen = wrap.classList.contains('open')
  closeAllDropdowns()
  if (!isOpen) wrap.classList.add('open')
}

// 规整状态文字
function polishStageText(file: any): string {
  const stage = file.polishStage || 'idle'
  if (stage === 'sending') return '发送中…'
  if (stage === 'receiving') return '接收中…'
  if (stage === 'failed') return '规整失败'
  return ''
}
</script>

<template>
  <div class="sidebar">
    <div class="sidebar-search">
      <svg class="search-icon" width="13" height="13" viewBox="0 0 13 13" fill="none">
        <circle cx="5.5" cy="5.5" r="4" stroke="currentColor" stroke-width="1.2"/>
        <path d="M8.5 8.5L12 12" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
      </svg>
      <input v-model="query" type="text" placeholder="搜索" class="search-input" />
    </div>

    <div class="file-list" @click="closeAllDropdowns">
      <div
        v-for="(f, i) in files" :key="f.id"
        class="file-item"
        :class="{ selected: f.id === store.selectedFileId }"
        :style="{ '--i': i }"
        @click="select(f.id)"
      >
        <div class="file-row-top">
          <span class="file-name" :title="f.name">{{ f.name }}</span>
          <button class="file-remove" @click="remove(f.id, $event)" title="移除">
            <svg width="8" height="8" viewBox="0 0 8 8"><path d="M1 1l6 6M7 1L1 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>
          </button>
        </div>

        <div class="file-row-meta">
          <span class="file-meta">{{ fmtDur(f.duration) }}</span>
          <span class="file-status" :class="f.error ? 's-error' : (f.status === 'completed' && f.polishStage === 'failed' ? 's-error' : statusMap[f.status]?.cls)">
            <template v-if="f.status === 'processing'">{{ processingStages[f.id] || '准备中' }}</template>
            <template v-else-if="f.status === 'transcribing'">转写中 {{ f.progress }}%</template>
            <template v-else-if="f.status === 'polishing'">{{ polishStageText(f) || '规整中' }}</template>
            <template v-else-if="f.status === 'failed'">转写失败</template>
            <template v-else-if="f.status === 'completed' && f.polishStage === 'failed'">规整失败</template>
            <template v-else>{{ statusMap[f.status]?.label }}</template>
          </span>
        </div>
        <div v-if="f.error" class="file-error" :title="f.error">{{ f.error }}</div>

        <div v-if="f.status === 'transcribing' || f.status === 'processing' || f.status === 'polishing'" class="file-progress">
          <div class="file-progress-bar" :style="{ width: f.progress + '%' }" />
        </div>

        <div class="file-actions">
          <!-- 转写按钮 -->
          <button
            class="action-btn"
            :class="{ disabled: f.status === 'transcribing' || f.status === 'processing' || f.status === 'polishing' }"
            @click.stop="showLanguageSelector(f.id)"
          >
            {{ (f.status === 'completed' || f.status === 'failed' || f.status === 'polishing') ? '重新转写' : '转写' }}
          </button>

          <!-- AI规整按钮 -->
          <button
            class="action-btn"
            :class="{ disabled: !f.transcription || f.status === 'transcribing' || f.status === 'processing' || f.status === 'polishing' || f.status === 'waiting' }"
            @click.stop="polishFile(f.id)"
          >
            {{ f.polished ? '重新规整' : 'AI 规整' }}
          </button>

          <!-- 导出按钮 -->
          <div class="export-wrap" @click.stop="toggleExport($event, f.id)">
            <button
              class="action-btn action-export"
              :class="{ disabled: f.status !== 'completed' || (!f.transcription && !f.polished) }"
            >导出</button>
            <div class="export-dropdown">
              <button v-if="f.transcription" class="export-item" @click.stop="exportFile(f.id, 'original')">导出原文</button>
              <button v-if="f.polished" class="export-item" @click.stop="exportFile(f.id, 'polished')">导出规整文本</button>
            </div>
          </div>
        </div>

        <!-- 语言选择弹窗 -->
        <Teleport to="body">
          <Transition name="modal">
            <div v-if="showLangMenu === f.id" class="lang-overlay" @click.stop="showLangMenu = null">
              <div class="lang-dialog" @click.stop>
                <div class="lang-dialog-title">指定语言（单选）</div>
                <div class="lang-dialog-options">
                  <button
                    v-for="lang in LANGUAGES" :key="lang.code"
                    class="lang-dialog-option"
                    :class="{ active: getSavedLang() === lang.code }"
                    @click.stop="confirmLanguage(f.id, lang.code)"
                  >
                    <span class="lang-dialog-label">{{ lang.label }}</span>
                    <svg v-if="getSavedLang() === lang.code" class="lang-dialog-check" width="14" height="14" viewBox="0 0 14 14" fill="none">
                      <path d="M3 7.5l3 3 5-6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                  </button>
                </div>
              </div>
            </div>
          </Transition>
        </Teleport>
      </div>

      <div v-if="files.length === 0" class="file-empty">
        {{ query ? '无匹配文件' : '点击左上角导入音频开始使用' }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.sidebar { width: 280px; background: var(--bg-surface); border-right: 1px solid var(--border-default); display: flex; flex-direction: column; flex-shrink: 0; }
.sidebar-search { padding: var(--space-3); border-bottom: 1px solid var(--border-default); position: relative; }
.search-icon { position: absolute; left: calc(var(--space-3) + 10px); top: 50%; transform: translateY(-50%); color: var(--text-disabled); pointer-events: none; }
.search-input { width: 100%; height: 32px; padding: 0 var(--space-2) 0 30px; border: 1px solid var(--border-default); border-radius: var(--radius-md); background: var(--bg-elevated); color: var(--text-primary); font-size: 12px; font-family: var(--font-sans); outline: none; transition: all 0.15s var(--ease-out); }
.search-input:focus { border-color: var(--accent); box-shadow: 0 0 0 2px rgba(110, 158, 255, 0.15); background: var(--bg-base); }
.search-input::placeholder { color: var(--text-disabled); }
.file-list { flex: 1; overflow-y: auto; padding: var(--space-2); display: flex; flex-direction: column; gap: var(--space-1); }

/* File item card */
.file-item {
  padding: var(--space-3);
  border-radius: var(--radius-lg);
  border: 1px solid transparent;
  cursor: pointer;
  transition: all 0.2s var(--ease-out);
  position: relative;
}
.file-item:hover {
  background: var(--bg-hover);
  border-color: var(--border-default);
}
.file-item.selected {
  background: var(--bg-overlay);
  border-color: rgba(110, 158, 255, 0.35);
  box-shadow: 0 0 0 1px rgba(110, 158, 255, 0.08), 0 2px 8px rgba(110, 158, 255, 0.06);
}
/* Gradient overlay on selected item */
.file-item.selected::before {
  content: '';
  position: absolute;
  inset: 0;
  background: linear-gradient(to right, rgba(110, 158, 255, 0.06), transparent);
  pointer-events: none;
  border-radius: inherit;
}

.file-row-top { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); position: relative; }
.file-name { flex: 1; font-size: 12px; font-weight: 500; color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.file-remove { width: 16px; height: 16px; border: none; background: transparent; color: var(--text-disabled); cursor: pointer; display: flex; align-items: center; justify-content: center; border-radius: var(--radius-sm); opacity: 0; transition: all var(--duration-fast) var(--ease-out); }
.file-item:hover .file-remove { opacity: 1; }
.file-remove:hover { color: var(--danger); background: var(--danger-muted); }
.file-row-meta { display: flex; align-items: center; justify-content: space-between; margin-top: var(--space-1); position: relative; }
.file-meta { font-size: 10px; font-family: var(--font-mono); color: var(--text-disabled); }
.file-status { font-size: 10px; font-weight: 500; letter-spacing: 0.02em; }
.s-waiting { color: var(--text-disabled); }
.s-active { color: var(--accent); }
.s-done { color: var(--success); }
.s-error { color: var(--danger); }
.file-error {
  font-size: 10px;
  color: var(--danger);
  margin-top: var(--space-1);
  line-height: 1.3;
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  word-break: break-all;
}
.file-progress { height: 2px; background: var(--border-default); border-radius: 1px; margin-top: var(--space-2); overflow: hidden; position: relative; }
.file-progress-bar { height: 100%; background: linear-gradient(90deg, var(--accent), rgba(110, 158, 255, 0.7)); border-radius: 1px; transition: width 0.3s var(--ease-out); }
.file-actions { display: flex; gap: var(--space-1); margin-top: var(--space-2); position: relative; }

/* Action buttons — ghost button style */
.action-btn {
  height: 28px;
  padding: 0 var(--space-3);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: transparent;
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 500;
  font-family: var(--font-sans);
  cursor: pointer;
  transition: all 0.15s var(--ease-out);
  white-space: nowrap;
}
.action-btn:hover { border-color: var(--border-strong); color: var(--text-primary); background: var(--bg-hover); }
.action-btn:active { transform: scale(0.97); }
.action-btn.disabled { opacity: 0.3; cursor: not-allowed; pointer-events: none; color: var(--text-disabled); }
.export-wrap { position: relative; }
.export-dropdown { display: none; position: absolute; bottom: 100%; left: 0; margin-bottom: var(--space-1); min-width: 120px; background: var(--bg-elevated); border: 1px solid var(--border-default); border-radius: var(--radius-md); padding: var(--space-1); z-index: 100; box-shadow: 0 4px 12px rgba(0,0,0,0.15); }
.export-wrap.open .export-dropdown { display: block; }
.export-item { width: 100%; height: 26px; padding: 0 var(--space-2); border: none; border-radius: var(--radius-sm); background: transparent; color: var(--text-primary); font-size: 11px; font-family: var(--font-sans); cursor: pointer; text-align: left; white-space: nowrap; transition: background var(--duration-fast) var(--ease-out); }
.export-item:hover { background: var(--bg-hover); }

/* 语言选择居中弹窗 */
.lang-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(4px);
  -webkit-backdrop-filter: blur(4px);
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
}
.lang-dialog {
  width: 240px;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.25);
  overflow: hidden;
}
.lang-dialog-title {
  height: 40px;
  padding: 0 var(--space-4);
  display: flex;
  align-items: center;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  border-bottom: 1px solid var(--border-default);
}
.lang-dialog-options {
  padding: var(--space-1);
}
.lang-dialog-option {
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
.lang-dialog-option:hover {
  background: var(--bg-hover);
}
.lang-dialog-option.active {
  color: var(--accent);
  font-weight: 500;
}
.lang-dialog-check {
  color: var(--accent);
  flex-shrink: 0;
}

/* modal transition */
.modal-enter-active .lang-dialog,
.modal-leave-active .lang-dialog {
  transition: transform 0.2s var(--ease-out), opacity 0.2s var(--ease-out);
}
.modal-enter-from .lang-dialog,
.modal-leave-to .lang-dialog {
  transform: scale(0.96);
  opacity: 0;
}
.modal-enter-from,
.modal-leave-to {
  opacity: 0;
}

.file-empty { padding: var(--space-8) var(--space-4); text-align: center; font-size: 12px; color: var(--text-disabled); }
</style>
