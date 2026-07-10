/**
 * Markdown → HTML 转换器
 * 基于 marked.js (MIT)，CommonMark + GFM 完整支持
 */

import { marked } from 'marked'

marked.setOptions({
  breaks: true,
  gfm: true,
})

export function markdownToHtml(md: string): string {
  if (!md) return ''
  return marked.parse(md, { async: false }) as string
}

/** HTML 实体转义（纯文本快速路径，跳过 marked.js） */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

const MARKDOWN_SYNTAX_RE = /[#*_`\[\]>|\-~\\]/

/** 检测文本增量是否需要 Markdown 解析（一旦含语法字符，需走 marked.js） */
export function needsMarkdownParsing(delta: string): boolean {
  return MARKDOWN_SYNTAX_RE.test(delta)
}
