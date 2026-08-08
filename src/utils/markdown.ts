/**
 * Markdown → HTML 转换器
 * 基于 marked.js (MIT)，CommonMark + GFM 完整支持
 * 清单 F-01：所有 markdown 渲染输出统一经 DOMPurify 消毒后再交给 v-html，
 * 禁止 on* 事件属性与 javascript:/data:text/html 等危险 URL（DOMPurify 默认行为，未放宽）。
 */

import { marked } from 'marked'
import DOMPurify from 'dompurify'

marked.setOptions({
  breaks: true,
  gfm: true,
})

// 外链需要 target 属性才能新开页面；DOMPurify 默认移除 target，
// 此处按官方推荐方式显式放行，并在消毒后强制补 rel="noopener noreferrer"，
// 防止 reverse tabnabbing。不放宽 on* 属性与 javascript: URL 拦截。
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A' && node.hasAttribute('target')) {
    node.setAttribute('rel', 'noopener noreferrer')
  }
})

/**
 * 全应用唯一的 HTML 消毒入口（清单 F-01）。
 * 任何进入 v-html 的 HTML（模型输出、拼接的帮助文案）都必须经过此函数。
 */
export function sanitizeHtml(html: string): string {
  if (!html) return ''
  return DOMPurify.sanitize(html, { ADD_ATTR: ['target'] })
}

export function markdownToHtml(md: string): string {
  if (!md) return ''
  const html = marked.parse(md, { async: false }) as string
  return sanitizeHtml(html)
}

/** HTML 实体转义（纯文本快速路径，跳过 marked.js） */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

// 清单 F-02（含 2026-08-04 全量评审补充）：收紧触发条件——
// `-`/`_`/`*`/`+`/`>` 仅在行首或空白之后才视为 Markdown 语法，
// 避免中文转写中的日期（2026-08-04）、编号（1-2）误触发全量 markdown 解析路径。
const MARKDOWN_SYNTAX_RE = /#|`|~|\||\\|(?:^|\s)[-_*+>]/m

/** 检测文本是否需要 Markdown 解析（用于判定渲染模式，流式阶段恒走纯文本，见 stores/files.ts） */
export function needsMarkdownParsing(text: string): boolean {
  return MARKDOWN_SYNTAX_RE.test(text)
}
