<script setup lang="ts">
import { useFilesStore } from '../stores/files'
import { useSettingsStore } from '../stores/settings'
import { useUiStore } from '../stores/ui'
import { usePromptsStore } from '../stores/prompts'
import { invoke } from '@tauri-apps/api/core'
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import SingleChoiceDialog from './SingleChoiceDialog.vue'
import PromptPickerDialog from './PromptPickerDialog.vue'
import ConfirmDialog from './ConfirmDialog.vue'
import type { FileInfo, PromptSnapshot, TaskOperation } from '../types'
import { useImportFlow } from '../composables/useImportFlow'
import { useExportFlow } from '../composables/useExportFlow'

const files = useFilesStore()
const settings = useSettingsStore()
const ui = useUiStore()
const prompts = usePromptsStore()
const importFlow = useImportFlow()
const exportFlow = useExportFlow()
const query = ref('')
const showLangMenu = ref<string | null>(null) // fileId or null
const pickerFileId = ref<string | null>(null)
const overwriteConfirm = ref<{ fileId: string; promptId: string } | null>(null)
const selectPending = ref(false)
const dragCounter = ref(0)
// B-04 双格式导出（清单 B-04）：待选择格式的导出请求
const exportPending = ref<{ fileId: string; type: 'original' | 'polished' } | null>(null)

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

const fileList = computed(() => {
  if (!query.value) return files.files
  const q = query.value.toLowerCase()
  return files.files.filter(f => f.name.toLowerCase().includes(q))
})

const statusMap: Record<string, { label: string; cls: string }> = {
  waiting: { label: '等待转写', cls: 's-waiting' },
  processing: { label: '准备中', cls: 's-active' },
  transcribing: { label: '转写中', cls: 's-active' },
  pausing: { label: '正在暂停', cls: 's-pausing' },
  paused: { label: '已暂停', cls: 's-paused' },
  polishing: { label: '规整中', cls: 's-active' },
  completed: { label: '已完成', cls: 's-done' },
  failed: { label: '失败', cls: 's-error' },
  cancelling: { label: '删除中', cls: 's-pausing' },
  cancelled: { label: '已取消', cls: 's-done' },
}

const unlistenFns: Array<() => void> = []

onMounted(async () => {
  // Tauri 原生拖拽：提供完整文件路径（process-status 已由事件层统一信封处理）
  const unlistenDrop = await getCurrentWindow().onDragDropEvent((event) => {
    if (event.payload.type === 'drop') {
      addDropFiles(event.payload.paths)
    }
  })
  unlistenFns.push(unlistenDrop)
})

onUnmounted(() => unlistenFns.forEach(fn => fn()))

function fmtDur(s: number) {
  if (!s) return '--:--'
  const m = Math.floor(s / 60)
  const sec = Math.floor(s % 60)
  return `${m}:${String(sec).padStart(2, '0')}`
}

function select(id: string) { closeAllDropdowns(); files.selectFile(id) }
async function remove(id: string, e: Event) {
  e.stopPropagation(); closeAllDropdowns()
  const file = files.files.find(f => f.id === id)
  if (!file) return
  if (file.status === 'cancelling') return
  const previousStatus = file.status

  // P1-02：删除进行中的文件不立即移除，先置"删除中"并取消任务，成功后再从列表移除
  const active = ['transcribing', 'processing', 'pausing', 'polishing', 'cancelling'].includes(file.status)
  if (active) {
    const op: TaskOperation = file.status === 'polishing' ? 'polish' : 'transcription'
    files.updateFile(id, { status: 'cancelling' })
    try {
      await files.requestCancelFile(id, op)
    } catch (err) {
      console.error('[删除] 取消失败:', err)
      files.updateFile(id, { status: previousStatus, error: '取消失败，文件未删除' })
      ui.showToastMessage('取消失败，文件未删除', 'error')
      return
    }
  } else if (file.status === 'paused') {
    try {
      await files.requestCancelFile(id, 'transcription')
    } catch (err) {
      console.error('[删除] 暂停任务取消失败:', err)
      files.updateFile(id, { error: '取消失败，文件未删除' })
      ui.showToastMessage('取消失败，文件未删除', 'error')
      return
    }
  }
  files.removeFile(id)
}

// 拖拽导入（音频 + 视频容器，视频由捆绑 ffmpeg 提取音轨）
function onSidebarDragEnter(e: DragEvent) { e.preventDefault(); dragCounter.value++ }
function onSidebarDragLeave(e: DragEvent) { e.preventDefault(); if (--dragCounter.value <= 0) dragCounter.value = 0 }
function onSidebarDrop(e: DragEvent) { e.preventDefault(); dragCounter.value = 0 }

// 拖拽只负责取得路径，统一交给 useImportFlow 处理（P5）
async function addDropFiles(paths: string[]) {
  await importFlow.importPaths(paths)
}
function closeAllDropdowns() {
  document.querySelectorAll('.export-wrap.open').forEach(el => el.classList.remove('open'))
  showLangMenu.value = null
}

// 显示语言选择
function showLanguageSelector(fileId: string) {
  closeAllDropdowns()
  const file = files.files.find(f => f.id === fileId)
  if (!file || file.status === 'transcribing' || file.status === 'processing' || file.status === 'polishing') return
  showLangMenu.value = fileId
}

// 确认语言后开始转写
function confirmLanguage(fileId: string, language: string) {
  showLangMenu.value = null
  saveLang(language)
  transcribeFile(fileId, language)
}

function onLangSelect(value: string) {
  const id = showLangMenu.value
  if (!id) return
  confirmLanguage(id, value)
}

async function transcribeFile(fileId: string, language: string) {
  const file = files.files.find(f => f.id === fileId)
  if (!file || file.status === 'transcribing' || file.status === 'processing' || file.status === 'pausing') return

  console.log(`[转写] 开始: ${file.name}, 语言=${language}`)
  // 新协议：前端生成 runId，后端返回 generation（不依赖 tempDir 断点参数）
  const runId = crypto.randomUUID()

  files.updateFile(fileId, {
    status: 'processing', progress: 0,
    transcription: '', polished: '',
    transcriptionWordCount: 0, polishedWordCount: 0,
    polishStage: 'idle', error: undefined,
    tempDir: undefined, lastCompletedIndex: undefined, resumeTotal: undefined,
    language,
  })
  // 新一轮转写：两面板渲染模式重置为纯文本（清单 F-02 方案 a）
  files.startTask(fileId, 'transcription', runId)
  ui.isTranscribing = true

  try {
    const generation = await invoke<number>('process_and_transcribe', {
      fileId,
      operation: 'transcription',
      runId,
      filePath: file.path,
      apiParams: {
        base_url: settings.asrBaseUrl,
        api_key: settings.asrKey,
        model: settings.apiConfig.transcriptionModel,
        language,
      },
    })
    files.setTaskGeneration(fileId, 'transcription', generation)
  } catch (e) {
    console.error(`[转写] 失败: ${file.name}`, e)
    const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
    files.updateFile(fileId, { status: 'failed', error: errMsg })
    ui.showToastMessage(`${file?.name || '文件'} 转写失败`, 'error')
  }

  ui.isTranscribing = files.hasActiveFiles
}

// 规整入口（F-06.6）：每次点击都先弹提示词选择菜单，不直接发起规整
async function openPolishPicker(fileId: string) {
  closeAllDropdowns()
  const file = files.files.find(f => f.id === fileId)
  if (!file || !file.transcription || file.status === 'polishing') return
  // 双保险：暂停态文件禁止发起规整（后端 start_chat 亦会拒绝）
  if (file.status === 'paused' || file.status === 'pausing') {
    ui.showToastMessage('该文件处于暂停状态，请先继续或取消转写', 'error')
    return
  }
  const ok = await prompts.ensureLoaded()
  if (!ok || !prompts.doc || prompts.prompts.length === 0) {
    ui.showToastMessage('无法加载提示词，请前往设置重试', 'error')
    return
  }
  pickerFileId.value = fileId
}

// 菜单中点击标题：已有规整结果先确认覆盖，再持久化选择并开始规整（F-06.6.5）
async function onPromptPicked(promptId: string) {
  const fileId = pickerFileId.value
  if (!fileId) return
  const file = files.files.find(f => f.id === fileId)
  if (!file) { pickerFileId.value = null; return }
  if (file.polished) {
    overwriteConfirm.value = { fileId, promptId }
    return
  }
  await selectAndPolish(fileId, promptId)
}

async function onOverwriteConfirm(value: string) {
  const pendingPick = overwriteConfirm.value
  overwriteConfirm.value = null
  if (value !== 'confirm' || !pendingPick) return
  await selectAndPolish(pendingPick.fileId, pendingPick.promptId)
}

// select_prompt 落盘成功后复制快照并开始规整；失败不清旧结果、不发请求
async function selectAndPolish(fileId: string, promptId: string) {
  if (selectPending.value) return
  selectPending.value = true
  try {
    const doc = await prompts.selectPrompt(promptId)
    const entry = doc.prompts.find(p => p.id === promptId)
    if (!entry) throw new Error('未找到提示词条目')
    pickerFileId.value = null
    polishFile(fileId, { id: entry.id, title: entry.title, content: entry.content })
  } catch (e) {
    console.error('[规整] 提示词选择保存失败:', e)
    ui.showToastMessage('提示词选择保存失败，未开始规整', 'error')
  } finally {
    selectPending.value = false
  }
}

// F-06.4.6：只使用传入的快照，避免异步期间串用新选择
async function polishFile(fileId: string, snapshot: PromptSnapshot) {
  closeAllDropdowns()
  const file = files.files.find(f => f.id === fileId)
  if (!file || !file.transcription || file.status === 'polishing') return

  console.log(`[规整] 开始: ${file.name}, 文本长度=${file.transcription.length}`)

  files.selectFile(fileId)
  files.updateFile(fileId, {
    polished: '', polishedWordCount: 0,
    status: 'polishing', polishStage: 'sending', error: undefined,
  })

  const runId = crypto.randomUUID()
  files.startTask(fileId, 'polish', runId)

  try {
    // B-11：start_chat IPC 参数聚合为 request 结构；新协议传 runId，返回 generation
    const generation = await invoke<number>('start_chat', {
      request: {
        fileId,
        fileName: file.name,
        systemPrompt: snapshot.content,
        userText: file.transcription,
        audioDataUrl: null,
        apiParams: {
          base_url: settings.polishBaseUrl,
          api_key: settings.polishKey,
          model: settings.committedConfig.polishModel,
          language: 'auto',
        },
        responseFormat: null,
      },
      runId,
    })
    files.setTaskGeneration(fileId, 'polish', generation)
  } catch (e) {
    console.error(`[规整] 失败: ${file.name}`, e)
    const errMsg = String(e).replace(/^Error invoking plugin /, '').slice(0, 200)
    files.updateFile(fileId, { status: 'completed', polishStage: 'failed', error: errMsg })
    ui.showToastMessage(`${file?.name || '文件'} 规整失败`, 'error')
  }
}

async function stopPolish(fileId: string) {
  closeAllDropdowns()
  const file = files.files.find(f => f.id === fileId)
  if (!file || file.status !== 'polishing') return
  files.updateFile(fileId, { status: 'cancelling' })
  try {
    await files.requestCancelFile(fileId, 'polish')
  } catch (e) {
    console.error('[规整] 停止取消失败:', e)
    files.updateFile(fileId, { status: 'completed', polishStage: 'failed', error: '取消失败' })
    ui.showToastMessage(`${file.name} 停止规整失败`, 'error')
    return
  }
  ui.showToastMessage(`${file.name} 已停止规整`, 'info')
}

async function exportFile(fileId: string, type: 'original' | 'polished') {
  closeAllDropdowns()
  const file = files.files.find(f => f.id === fileId)
  if (!file) return
  const content = type === 'original' ? file.transcription : file.polished
  if (!content) {
    ui.showToastMessage(`${file.name} ${type === 'original' ? '暂无原文' : '暂无规整结果'}`, 'error')
    return
  }
  // 先弹格式选择（txt/md 双格式可达，清单 B-04），选中后再统一走 useExportFlow 导出
  exportPending.value = { fileId, type }
}

function onExportFormatSelect(value: string) {
  const pending = exportPending.value
  exportPending.value = null
  if (!pending || (value !== 'txt' && value !== 'md')) return
  void doExport(pending.fileId, pending.type, value)
}

function getExportContent(fileId: string, type: 'original' | 'polished'): string {
  const file = files.files.find(f => f.id === fileId)
  if (!file) return ''
  return type === 'original' ? file.transcription : file.polished
}

function getExportName(fileId: string, type: 'original' | 'polished'): string {
  const file = files.files.find(f => f.id === fileId)
  if (!file) return 'export'
  const suffix = type === 'original' ? '_原文' : '_规整'
  return `${file.name.replace(/\.[^.]+$/, '')}${suffix}`
}

// P6：导出单飞委托 useExportFlow（UTF-8 bytes 上限校验 + 按 fileId 锁定）
function doExport(fileId: string, type: 'original' | 'polished', format: 'txt' | 'md') {
  return exportFlow.exportFile(fileId, type, format, getExportContent, getExportName)
}

function toggleExport(event: Event, fileId: string) {
  const file = files.files.find(f => f.id === fileId)
  if (!file || file.status !== 'completed' || (!file.transcription && !file.polished)) return
  if (exportFlow.isExporting(fileId)) return
  const wrap = event.currentTarget as HTMLElement
  const isOpen = wrap.classList.contains('open')
  closeAllDropdowns()
  if (!isOpen) wrap.classList.add('open')
}

// 规整状态文字
function polishStageText(file: FileInfo): string {
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

    <div class="file-list" @click="closeAllDropdowns" @dragenter="onSidebarDragEnter" @dragleave="onSidebarDragLeave" @dragover.prevent @drop="onSidebarDrop">
      <div v-if="dragCounter > 0" class="drag-overlay">
        <svg width="24" height="24" viewBox="0 0 24 24" fill="none"><path d="M12 3v12M8 11l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><rect x="3" y="16" width="18" height="5" rx="2" stroke="currentColor" stroke-width="1.5"/></svg>
        <span>将文件拖动到此处加入队列</span>
      </div>
      <div
        v-for="(f, i) in fileList" :key="f.id"
        class="file-item"
        :class="{ selected: f.id === files.selectedFileId }"
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
            <template v-if="f.status === 'processing'">{{ f.stage || '准备中' }}</template>
            <template v-else-if="f.status === 'transcribing'">转写中 {{ f.progress }}%</template>
            <template v-else-if="f.status === 'polishing'">{{ polishStageText(f) || '规整中' }}</template>
            <template v-else-if="f.status === 'failed'">转写失败</template>
            <template v-else-if="f.status === 'completed' && f.polishStage === 'failed'">规整失败</template>
            <template v-else>{{ statusMap[f.status]?.label }}</template>
          </span>
        </div>
        <div v-if="f.error" class="file-error" :title="f.error">{{ f.error }}</div>

        <div v-if="['transcribing','pausing','paused'].includes(f.status)" class="file-progress">
          <div class="file-progress-bar" :style="{ width: f.progress + '%' }" />
        </div>

        <div class="file-actions">
          <!-- 暂停中：黄色提示，不可点击 -->
          <button v-if="f.status === 'pausing'"
            class="action-btn btn-pausing" disabled>
            正在暂停
          </button>
          <!-- 已暂停：绿色继续按钮 -->
          <button v-else-if="f.status === 'paused'"
            class="action-btn btn-primary"
            @click.stop="files.resumeTranscription(f.id)">
            继续转写
          </button>
          <!-- 转写中：红色暂停按钮 -->
          <button v-else-if="f.status === 'transcribing'"
            class="action-btn btn-pause"
            @click.stop="files.pauseTranscription(f.id)">
            暂停转写
          </button>
          <!-- 默认：转写/重新转写 -->
          <button v-else
            class="action-btn btn-primary"
            :class="{ disabled: f.status === 'processing' || f.status === 'polishing' }"
            @click.stop="showLanguageSelector(f.id)"
          >
            {{ (f.status === 'completed' || f.status === 'failed' || f.status === 'polishing') ? '重新转写' : '转写' }}
          </button>

          <!-- AI规整 / 停止规整 -->
          <button v-if="f.status === 'polishing'"
            class="action-btn btn-pause"
            @click.stop="stopPolish(f.id)"
          >
            停止规整
          </button>
          <button v-else
            class="action-btn"
            :class="{ disabled: !f.transcription || ['transcribing','processing','pausing','paused','waiting'].includes(f.status) || prompts.loadFailed }"
            @click.stop="openPolishPicker(f.id)"
          >
            {{ f.polished ? '重新规整' : 'AI 规整' }}
          </button>

          <!-- 导出按钮 -->
          <div class="export-wrap" @click.stop="toggleExport($event, f.id)">
            <button
              class="action-btn action-export"
              :class="{ disabled: !['completed','paused'].includes(f.status) || (!f.transcription && !f.polished) || exportFlow.isExporting(f.id) }"
            >{{ exportFlow.isExporting(f.id) ? '导出中' : '导出' }}</button>
            <div class="export-dropdown">
              <button v-if="f.transcription" class="export-item" :class="{ 'export-item-disabled': exportFlow.isExporting(f.id) }" @click.stop="exportFile(f.id, 'original')">导出原文</button>
              <button v-if="f.polished" class="export-item" :class="{ 'export-item-disabled': exportFlow.isExporting(f.id) }" @click.stop="exportFile(f.id, 'polished')">导出规整文本</button>
            </div>
          </div>
        </div>

      </div>

      <div v-if="fileList.length === 0" class="file-empty">
        {{ query ? '无匹配文件' : '点击左上角导入音频/视频开始使用' }}
      </div>
    </div>

    <!-- 语言选择（共享单选弹层，清单 F-06.6.2 抽取） -->
    <SingleChoiceDialog
      :visible="showLangMenu !== null"
      title="指定语言（单选）"
      :options="LANGUAGES.map(l => ({ value: l.code, label: l.label }))"
      :active-value="getSavedLang()"
      highlight="check"
      @select="onLangSelect"
      @close="showLangMenu = null"
    />

    <!-- 规整提示词按次选择（F-06.6） -->
    <PromptPickerDialog
      :visible="pickerFileId !== null"
      @select="onPromptPicked"
      @close="pickerFileId = null"
    />

    <!-- 覆盖现有规整结果确认 -->
    <ConfirmDialog
      :visible="overwriteConfirm !== null"
      title="覆盖规整结果"
      message="该文件已有规整结果，重新规整将覆盖现有内容。是否继续？"
      :actions="[
        { value: 'cancel', label: '取消' },
        { value: 'confirm', label: '覆盖并规整', variant: 'danger' },
      ]"
      @select="onOverwriteConfirm"
      @close="overwriteConfirm = null"
    />

    <!-- 导出格式选择（清单 B-04：txt/md 双格式可达；原文默认 txt，规整结果默认 md） -->
    <SingleChoiceDialog
      :visible="exportPending !== null"
      title="选择导出格式（单选）"
      :options="[
        { value: 'txt', label: 'TXT 纯文本' },
        { value: 'md', label: 'Markdown (.md)' },
      ]"
      :active-value="exportPending?.type === 'polished' ? 'md' : 'txt'"
      highlight="check"
      @select="onExportFormatSelect"
      @close="exportPending = null"
    />
  </div>
</template>

<style scoped>
.sidebar { width: 280px; background: var(--bg-surface); border-right: 1px solid var(--border-default); display: flex; flex-direction: column; flex-shrink: 0; }
.sidebar-search { padding: var(--space-3); border-bottom: 1px solid var(--border-default); position: relative; }
.search-icon { position: absolute; left: calc(var(--space-3) + 10px); top: 50%; transform: translateY(-50%); color: var(--text-disabled); pointer-events: none; }
.search-input { width: 100%; height: 32px; padding: 0 var(--space-2) 0 30px; border: 1px solid var(--border-default); border-radius: var(--radius-md); background: var(--bg-elevated); color: var(--text-primary); font-size: 12px; font-family: var(--font-sans); outline: none; transition: all 0.15s var(--ease-out); }
.search-input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px rgba(110, 158, 255, 0.1); background: var(--bg-base); }
.search-input::placeholder { color: var(--text-disabled); }
.file-list { flex: 1; overflow-y: auto; overflow-x: hidden; padding: var(--space-2); display: flex; flex-direction: column; gap: var(--space-1); position: relative; }

/* File item card */
.file-item {
  padding: var(--space-3);
  border-radius: var(--radius-lg);
  border: 1px solid transparent;
  cursor: pointer;
  transition: all 0.2s var(--ease-out);
  position: relative;
  max-width: 100%;
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
/* 左侧蓝色高亮条 */
.file-item.selected::before {
  content: '';
  position: absolute;
  left: 0;
  top: 8px;
  bottom: 8px;
  width: 3px;
  background: var(--accent);
  border-radius: 0 2px 2px 0;
  pointer-events: none;
}

.file-row-top { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); position: relative; }
.file-name { flex: 1; font-size: 12px; font-weight: 500; color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.file-remove { width: 16px; height: 16px; border: none; background: transparent; color: var(--text-disabled); cursor: pointer; display: flex; align-items: center; justify-content: center; border-radius: var(--radius-sm); opacity: 0; transition: all var(--duration-fast) var(--ease-out); }
.file-item:hover .file-remove { opacity: 1; }
.file-remove:hover { color: var(--danger); background: var(--danger-muted); }
.file-row-meta { display: flex; align-items: center; justify-content: space-between; margin-top: var(--space-1); position: relative; }
.file-meta { font-size: 10px; font-family: var(--font-mono); color: var(--text-disabled); }

/* 状态标签 */
.file-status { font-size: 10px; font-weight: 500; letter-spacing: 0.03em; display: inline-flex; align-items: center; gap: 5px; }
.file-status::before {
  content: '';
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}
.s-waiting { color: var(--text-disabled); }
.s-waiting::before { background: var(--text-disabled); }
.s-active { color: var(--accent); }
.s-active::before { background: var(--accent); animation: status-pulse 1.5s ease-in-out infinite; }
.s-done { color: var(--success); }
.s-done::before { background: var(--success); }
.s-error { color: var(--danger); }
.s-error::before { background: var(--danger); }
.s-pausing { color: var(--warning); }
.s-pausing::before { background: var(--warning); animation: status-fade 2s ease-in-out infinite; }
.s-paused { color: var(--warning); }
.s-paused::before { background: var(--warning); }

@keyframes status-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
@keyframes status-fade {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.55; }
}

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
.file-progress-bar {
  height: 100%;
  background: linear-gradient(90deg, var(--accent), rgba(110, 158, 255, 0.6));
  border-radius: 1px;
  transition: width 0.3s var(--ease-out);
  position: relative;
}
.file-progress-bar::after {
  content: '';
  position: absolute;
  inset: 0;
  background: linear-gradient(90deg, transparent 0%, rgba(255,255,255,0.12) 50%, transparent 100%);
  animation: shimmer 1.5s infinite;
}
@keyframes shimmer {
  from { transform: translateX(-100%); }
  to { transform: translateX(100%); }
}
.file-actions { display: flex; gap: 6px; margin-top: var(--space-2); position: relative; }

/* Action buttons */
.action-btn {
  height: 26px;
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
/* 主操作：转写/重新转写 */
.action-btn.btn-primary { color: var(--accent); border-color: var(--accent); }
.action-btn.btn-primary:hover { background: var(--accent-muted); border-color: var(--accent-hover); }
/* 暂停 */
.btn-pause { color: var(--danger); border-color: var(--danger); }
.btn-pause:hover { background: var(--danger-muted); border-color: var(--danger); }
/* 正在暂停 */
.btn-pausing { color: var(--warning); border-color: var(--warning); opacity: 0.5; cursor: not-allowed; pointer-events: none; }
/* disabled */
.action-btn.disabled { opacity: 0.25; cursor: not-allowed; pointer-events: none; color: var(--text-disabled); }

.export-wrap { position: relative; }
.export-dropdown { position: absolute; bottom: 100%; left: 0; margin-bottom: var(--space-1); min-width: 100px; background: var(--bg-surface); border: 1px solid var(--border-default); border-radius: var(--radius-md); padding: var(--space-1); z-index: 200; box-shadow: 0 8px 32px rgba(0,0,0,0.25); opacity: 0; transform: translateY(4px) scale(0.97); pointer-events: none; transition: opacity 0.15s var(--ease-out), transform 0.15s var(--ease-out); }
.export-wrap.open .export-dropdown { opacity: 1; transform: translateY(0) scale(1); pointer-events: auto; }
.export-item { width: 100%; height: 26px; padding: 0 var(--space-2); border: none; border-radius: var(--radius-sm); background: transparent; color: var(--text-primary); font-size: 11px; font-family: var(--font-sans); cursor: pointer; text-align: left; white-space: nowrap; transition: background var(--duration-fast) var(--ease-out); }
.export-item:hover { background: var(--bg-hover); }
.export-item-disabled { opacity: 0.4; pointer-events: none; }

.file-empty { padding: var(--space-8) var(--space-4); text-align: center; font-size: 12px; color: var(--text-disabled); }

.drag-overlay {
  position: absolute;
  inset: 0;
  background: var(--bg-surface);
  border: 2px dashed var(--accent);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-2);
  color: var(--accent);
  font-size: 12px;
  font-weight: 500;
  z-index: 10;
  pointer-events: none;
  animation: drag-fade-in 0.15s var(--ease-out);
}
@keyframes drag-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
</style>
