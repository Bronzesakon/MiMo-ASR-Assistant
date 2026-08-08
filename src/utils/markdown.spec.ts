// 清单 F-01 验收测试：Markdown 消毒工具（DOMPurify 统一消毒入口）
// 覆盖：<script>、事件属性、javascript:/data: URL、SVG/MathML、畸形嵌套标签、
// Markdown 链接与图片；并断言普通标题、列表、表格、代码块渲染不受影响。
import { describe, it, expect } from 'vitest'
import { markdownToHtml, sanitizeHtml, escapeHtml, needsMarkdownParsing } from './markdown'

describe('markdownToHtml XSS 消毒（清单 F-01）', () => {
  it('移除 <script> 标签', () => {
    const html = markdownToHtml('正常文本 <script>alert(1)</script> 结尾')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('</script')
    expect(html).toContain('正常文本')
  })

  it('移除 onerror/onclick 等事件属性', () => {
    const img = markdownToHtml('<img src=x onerror=alert(1)>')
    expect(img).not.toContain('onerror')
    expect(img).not.toContain('alert(1)')

    const div = markdownToHtml('<div onclick="alert(1)">点我</div>')
    expect(div).not.toContain('onclick')
    expect(div).toContain('点我')
  })

  it('清除 javascript: 与 data:text/html 链接', () => {
    const jsLink = markdownToHtml('[点我](javascript:alert(1))')
    expect(jsLink).not.toContain('javascript:')

    const dataLink = markdownToHtml('[点我](data:text/html,<script>alert(1)</script>)')
    expect(dataLink).not.toContain('data:text/html')
    expect(dataLink).not.toContain('<script')
  })

  it('中和 SVG 注入（脚本与事件属性）', () => {
    const svgScript = markdownToHtml('<svg><script>alert(1)</script></svg>')
    expect(svgScript).not.toContain('<script')

    const svgEvent = markdownToHtml('<svg onload=alert(1)></svg>')
    expect(svgEvent).not.toContain('onload')
    expect(svgEvent).not.toContain('alert(1)')
  })

  it('中和 MathML/mXSS 混合载荷', () => {
    const mxss = markdownToHtml(
      '<math><mtext><table><mglyph><style><!--</style><img src=x onerror=alert(1)>'
    )
    expect(mxss).not.toContain('onerror')
    expect(mxss).not.toContain('<style')
  })

  it('处理畸形嵌套标签不残留危险节点', () => {
    const malformed = markdownToHtml('<<script>script>alert(1)<</script/script>')
    expect(malformed).not.toContain('<script')
    expect(malformed).not.toMatch(/\son\w+\s*=/)

    const nested = markdownToHtml('<di<b onclick=alert(1)>v>内容</b>')
    expect(nested).not.toContain('onclick')
  })

  it('移除 iframe/object/form 等非白名单危险标签', () => {
    const html = markdownToHtml('<iframe src="https://evil.example"></iframe><object data="x"></object>')
    expect(html).not.toContain('<iframe')
    expect(html).not.toContain('<object')
  })
})

describe('markdownToHtml 正常渲染不被消毒破坏', () => {
  it('标题、列表、链接、图片正常', () => {
    const html = markdownToHtml('# 标题\n\n- 项目一\n- 项目二\n\n[官网](https://example.com)\n\n![图](https://example.com/a.png)')
    expect(html).toContain('<h1>标题</h1>')
    expect(html).toContain('<li>项目一</li>')
    expect(html).toContain('<a href="https://example.com">官网</a>')
    expect(html).toContain('<img src="https://example.com/a.png"')
  })

  it('表格与代码块正常', () => {
    const html = markdownToHtml('| 列A | 列B |\n| --- | --- |\n| 1 | 2 |\n\n```js\nconst a = 1\n```')
    expect(html).toContain('<table>')
    expect(html).toContain('<th>列A</th>')
    expect(html).toContain('<td>1</td>')
    expect(html).toContain('<pre>')
    expect(html).toContain('<code')
    expect(html).toContain('const a = 1')
  })

  it('think 过滤后的纯文本内容正常成段', () => {
    const html = markdownToHtml('这是过滤后的正文，第一段。\n第二段内容。')
    expect(html).toContain('<p>这是过滤后的正文，第一段。<br>第二段内容。</p>')
  })
})

describe('sanitizeHtml 统一消毒入口', () => {
  it('直接消毒 HTML 片段', () => {
    expect(sanitizeHtml('<b>加粗</b><img src=x onerror=alert(1)>')).toBe('<b>加粗</b><img src="x">')
    expect(sanitizeHtml('')).toBe('')
  })

  it('保留安全链接的 target/rel', () => {
    const html = sanitizeHtml('<a href="https://example.com" target="_blank" rel="noopener noreferrer">链</a>')
    expect(html).toContain('href="https://example.com"')
    expect(html).toContain('target="_blank"')
  })
})

describe('escapeHtml 纯文本路径', () => {
  it('实体转义全部危险字符', () => {
    const escaped = escapeHtml('<script>alert(1)</script> & "quote"')
    expect(escaped).toBe('&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;quote&quot;')
  })
})

describe('needsMarkdownParsing 收紧后的触发条件（清单 F-02）', () => {
  it('日期与编号不误触发', () => {
    expect(needsMarkdownParsing('2026-08-04 今天开了项目评审会')).toBe(false)
    expect(needsMarkdownParsing('第1-2章的内容，参考3_4节')).toBe(false)
    expect(needsMarkdownParsing('普通的中文转写文本，只有标点。')).toBe(false)
  })

  it('真实 Markdown 语法触发', () => {
    expect(needsMarkdownParsing('# 标题')).toBe(true)
    expect(needsMarkdownParsing('段落结尾\n- 列表项')).toBe(true)
    expect(needsMarkdownParsing('加粗 **重点** 内容')).toBe(true)
    expect(needsMarkdownParsing('代码 `npm install` 示例')).toBe(true)
    expect(needsMarkdownParsing('引用\n> 内容')).toBe(true)
    expect(needsMarkdownParsing('| 表格 | 列 |')).toBe(true)
  })
})
