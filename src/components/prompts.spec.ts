// 清单 F-06.9 前端验收测试：组件 DOM/布局契约
// 无 @vue/test-utils 依赖，直接 createApp 挂载 + jsdom 断言（清单边界：不新增依赖）。
// 注：jsdom 不应用组件 scoped CSS 也不做布局，样式声明改用源码规则断言，
// 布局尺寸类断言（scrollHeight===clientHeight）列入「待人工视觉确认」。
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createApp, type App } from 'vue'
import { createPinia, setActivePinia, type Pinia } from 'pinia'
import type { Component } from 'vue'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => {
    throw new Error('invoke not mocked')
  }),
}))

// jsdom 未实现 matchMedia（settings store 主题监听依赖）
Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockReturnValue({
    matches: false,
    media: '',
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }),
})

import PromptRow from './PromptRow.vue'
import PromptPickerDialog from './PromptPickerDialog.vue'
import { usePromptsStore } from '../stores/prompts'
import type { PromptEntry } from '../types'

// 通过 Vite raw 导入获取组件源码做样式声明断言（避免依赖 node 类型，与 vitest 转换管线一致）
const rawSources = import.meta.glob('./*.vue', { query: '?raw', import: 'default', eager: true }) as Record<string, string>
const promptRowCss = rawSources['./PromptRow.vue']

function mount(component: Component, props: Record<string, unknown>, pinia?: Pinia): { el: HTMLElement; app: App } {
  const el = document.createElement('div')
  document.body.appendChild(el)
  const app = createApp(component, props)
  app.use(pinia ?? createPinia())
  app.mount(el)
  return { el, app }
}

function unmount(app: App, el: HTMLElement) {
  app.unmount()
  el.remove()
  document.querySelectorAll('.sc-overlay, .cd-overlay').forEach(n => n.remove())
}

function entry(id: string, title: string, isDefault = false): PromptEntry {
  return { id, title, content: `正文-${id}`, is_default: isDefault }
}

const rowBaseProps = {
  expanded: false,
  dragDisabled: false,
  isDragging: false,
  dropPosition: null,
  draftTitle: '',
  draftContent: '',
  errorMsg: '',
  saving: false,
}

describe('PromptRow 收起横条契约', () => {
  it('DOM 顺序：标题 → 编辑 → 删除 → 上移 → 下移，且不启用拖拽', () => {
    const { el, app } = mount(PromptRow, { ...rowBaseProps, entry: entry('a', '自定义条目') })
    const bar = el.querySelector('.prow-bar')!
    const title = bar.querySelector('.prow-title')!
    const editBtn = el.querySelector('[data-testid="prompt-edit"]')!
    const deleteBtn = el.querySelector('[data-testid="prompt-delete"]')!
    const moveUp = el.querySelector('[data-testid="prompt-move-up"]')!
    const moveDown = el.querySelector('[data-testid="prompt-grip"]')!

    const POS = Node.DOCUMENT_POSITION_FOLLOWING
    expect(title.compareDocumentPosition(editBtn) & POS).toBeTruthy()
    expect(editBtn.compareDocumentPosition(deleteBtn) & POS).toBeTruthy()
    expect(deleteBtn.compareDocumentPosition(moveUp) & POS).toBeTruthy()
    expect(moveUp.compareDocumentPosition(moveDown) & POS).toBeTruthy()

    // 下移按钮是横条最后一个操作元素
    const actions = Array.from(bar.querySelectorAll('.prow-btn'))
    expect(actions[actions.length - 2]).toBe(moveUp)
    expect(actions[actions.length - 1]).toBe(moveDown)

    expect(moveDown.getAttribute('draggable')).toBe('false')
    expect(bar.getAttribute('draggable')).toBeNull()
    expect(editBtn.getAttribute('draggable')).toBeNull()
    expect(deleteBtn.getAttribute('draggable')).toBeNull()
    expect(title.getAttribute('draggable')).toBeNull()
    unmount(app, el)
  })

  it('默认项不渲染删除按钮，其它按钮尺寸/存在性不受影响', () => {
    const { el, app } = mount(PromptRow, { ...rowBaseProps, entry: entry('builtin-default', '默认', true) })
    expect(el.querySelector('[data-testid="prompt-delete"]')).toBeNull()
    expect(el.querySelector('[data-testid="prompt-edit"]')).not.toBeNull()
    expect(el.querySelector('[data-testid="prompt-grip"]')).not.toBeNull()
    unmount(app, el)
  })

  it('标题容器 min-width:0 且单行省略（样式声明 + 类名）', () => {
    const { el, app } = mount(PromptRow, { ...rowBaseProps, entry: entry('a', '很长的标题'.repeat(8)) })
    const title = el.querySelector('.prow-title')!
    expect(title).not.toBeNull()
    // jsdom 不应用 scoped CSS：断言组件源码中 .prow-title 规则包含省略三件套
    const rule = promptRowCss.match(/\.prow-title\s*\{[^}]*\}/)?.[0] ?? ''
    expect(rule).toContain('min-width: 0')
    expect(rule).toContain('overflow: hidden')
    expect(rule).toContain('text-overflow: ellipsis')
    expect(rule).toContain('white-space: nowrap')
    unmount(app, el)
  })

  it('编辑状态禁用时上下移按钮均禁用', () => {
    const { el, app } = mount(PromptRow, { ...rowBaseProps, entry: entry('a', '条目'), dragDisabled: true })
    expect(el.querySelector('[data-testid="prompt-move-up"]')!.hasAttribute('disabled')).toBe(true)
    expect(el.querySelector('[data-testid="prompt-grip"]')!.hasAttribute('disabled')).toBe(true)
    unmount(app, el)
  })
})

describe('PromptRow 展开编辑器契约（F-06.5.9）', () => {
  it('默认项：不渲染标题输入框，显示「恢复默认」', () => {
    const { el, app } = mount(PromptRow, {
      ...rowBaseProps,
      entry: entry('builtin-default', '默认', true),
      expanded: true,
      draftContent: '正文',
    })
    expect(el.querySelector('[data-testid="prompt-title-input"]')).toBeNull()
    expect(el.querySelector('[data-testid="prompt-restore-default"]')).not.toBeNull()
    expect(el.querySelector('[data-testid="prompt-save"]')).not.toBeNull()
    unmount(app, el)
  })

  it('自定义项：渲染标题输入框，无「恢复默认」', () => {
    const { el, app } = mount(PromptRow, {
      ...rowBaseProps,
      entry: entry('a', '自定义'),
      expanded: true,
      draftTitle: '自定义',
      draftContent: '正文',
    })
    expect(el.querySelector('[data-testid="prompt-title-input"]')).not.toBeNull()
    expect(el.querySelector('[data-testid="prompt-restore-default"]')).toBeNull()
    unmount(app, el)
  })

  it('字符计数显示，超限触发 over 标记', () => {
    const { el, app } = mount(PromptRow, {
      ...rowBaseProps,
      entry: entry('a', '自定义'),
      expanded: true,
      draftContent: '字'.repeat(4001),
    })
    expect(el.querySelector('.prow-count')!.textContent).toContain('4001 / 4000')
    expect(el.querySelector('.prow-count.over')).not.toBeNull()
    unmount(app, el)
  })
})

describe('PromptPickerDialog 菜单契约（F-06.6.3）', () => {
  let pinia: Pinia

  beforeEach(() => {
    pinia = createPinia()
    setActivePinia(pinia)
  })

  function setDoc(count: number) {
    const store = usePromptsStore(pinia)
    const prompts: PromptEntry[] = [entry('builtin-default', '默认', true)]
    for (let i = 1; i < count; i++) prompts.push(entry(`p${i}`, `提示词${i}`))
    store.doc = { schema_version: 1, selected_prompt_id: 'builtin-default', prompts }
    return store
  }

  it('1 条：选项数与顺序与文档一致，仅显示标题不含正文', () => {
    setDoc(1)
    const { el, app } = mount(PromptPickerDialog, { visible: true }, pinia)
    const overlay = document.querySelector('.sc-overlay')!
    const options = Array.from(overlay.querySelectorAll('.sc-dialog-option'))
    expect(options.length).toBe(1)
    expect(options[0].textContent).toContain('默认')
    expect(overlay.textContent).not.toContain('正文-')
    unmount(app, el)
  })

  it('8 条：选项数与顺序与文档一致，无内部滚动声明', () => {
    setDoc(8)
    const { el, app } = mount(PromptPickerDialog, { visible: true }, pinia)
    const overlay = document.querySelector('.sc-overlay')!
    const options = Array.from(overlay.querySelectorAll('.sc-dialog-option'))
    expect(options.length).toBe(8)
    expect(options[0].textContent).toContain('默认')
    expect(options[1].textContent).toContain('提示词1')
    expect(options[7].textContent).toContain('提示词7')
    // 菜单 DOM 不包含提示词正文
    expect(overlay.textContent).not.toContain('正文-')
    // 选项容器不声明 max-height/overflow（真实无滚动条待人工视觉确认）
    const pickerCss = rawSources['./SingleChoiceDialog.vue']
    const optionsRule = pickerCss.match(/\.sc-dialog-options\s*\{[^}]*\}/)?.[0] ?? ''
    expect(optionsRule).not.toContain('max-height')
    expect(optionsRule).not.toContain('overflow')
    unmount(app, el)
  })

  it('最近项只产生背景高亮类，不显示勾选图标或「最近」标签', () => {
    setDoc(3)
    const { el, app } = mount(PromptPickerDialog, { visible: true }, pinia)
    const overlay = document.querySelector('.sc-overlay')!
    const options = Array.from(overlay.querySelectorAll('.sc-dialog-option'))
    const recent = options.find(o => o.classList.contains('recent'))
    expect(recent).toBeTruthy()
    expect(recent!.textContent).toContain('默认')
    expect(recent!.querySelector('svg')).toBeNull()
    expect(overlay.textContent).not.toContain('最近')
    expect(overlay.querySelectorAll('.sc-dialog-check').length).toBe(0)
    unmount(app, el)
  })
})

describe('PromptManager 状态契约（F-06.5）', () => {
  it('同时最多一个编辑区展开（展开由单一 draft 状态驱动）', async () => {
    const pinia = createPinia()
    setActivePinia(pinia)
    const store = usePromptsStore(pinia)
    store.doc = {
      schema_version: 1,
      selected_prompt_id: 'builtin-default',
      prompts: [entry('builtin-default', '默认', true), entry('a', '条目A'), entry('b', '条目B')],
    }
    const { default: PromptManager } = await import('./PromptManager.vue')
    const { el, app } = mount(PromptManager, {}, pinia)
    // 模拟加载完成后的列表
    await new Promise(r => setTimeout(r, 0))

    // 点击两个编辑按钮，任意时刻最多一个编辑器
    const editButtons = Array.from(el.querySelectorAll('[data-testid="prompt-edit"]')) as HTMLElement[]
    editButtons[1].click()
    await new Promise(r => setTimeout(r, 0))
    expect(el.querySelectorAll('[data-testid="prompt-editor"]').length).toBe(1)

    // 第二条未保存变更 → 弹三选确认而不是直接切换（编辑器仍只有一个）
    const titleInput = el.querySelector('[data-testid="prompt-title-input"]') as HTMLInputElement
    titleInput.value = '已修改'
    titleInput.dispatchEvent(new Event('input'))
    editButtons[2].click()
    await new Promise(r => setTimeout(r, 0))
    expect(el.querySelectorAll('[data-testid="prompt-editor"]').length).toBe(1)
    expect(document.querySelector('.cd-dialog')).not.toBeNull()
    unmount(app, el)
  })
})
