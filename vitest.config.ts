// 清单 F-01/F-06：前端最小测试环境（vitest + jsdom），覆盖 Markdown 消毒工具
// 与提示词组件 DOM/布局契约（挂载 .vue SFC 需要 @vitejs/plugin-vue，已有依赖）
import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'jsdom',
    include: ['src/**/*.spec.ts'],
  },
})
