// 跨 shell 的 build 脚本：先类型检查，再 vite build。
// 背景：.npmrc 指定 script-shell=powershell，而 PowerShell 5.1 不支持 `&&`，
// 因此不能在 npm scripts 里直接写 "vue-tsc --noEmit && vite build"。
// 用法：npm run build（等价于 vue-tsc --noEmit 成功后 vite build）
import { spawnSync } from 'node:child_process'

function run(args) {
  const result = spawnSync(process.execPath, args, { stdio: 'inherit' })
  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}

// 直接用 node 调本地 bin，避免 npx/cmd shim 在不同 shell 下的差异
run(['node_modules/vue-tsc/bin/vue-tsc.js', '--noEmit'])
run(['node_modules/vite/bin/vite.js', 'build'])
