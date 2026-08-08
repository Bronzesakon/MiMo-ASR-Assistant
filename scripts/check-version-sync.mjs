// 版本号同步检查：package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml 三处必须一致。
// 本脚本只读取 src-tauri 下的文件做核对，不修改任何 Rust/Tauri 文件。
// 用法：npm run version:check
// 发版时统一使用 `npm run tauri version <x.y.z>` 同时更新三处（详见 README「版本发布」）。
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

function readJsonVersion(relPath) {
  const file = join(root, relPath)
  return JSON.parse(readFileSync(file, 'utf-8')).version
}

function readCargoVersion(relPath) {
  const file = join(root, relPath)
  const content = readFileSync(file, 'utf-8')
  // 只取第一个顶层 version = "x.y.z"（[package] 段）
  const match = content.match(/^version\s*=\s*"([^"]+)"/m)
  if (!match) {
    console.error(`[版本同步] 无法在 ${relPath} 中解析 version 字段`)
    process.exit(1)
  }
  return match[1]
}

const versions = {
  'package.json': readJsonVersion('package.json'),
  'src-tauri/tauri.conf.json': readJsonVersion('src-tauri/tauri.conf.json'),
  'src-tauri/Cargo.toml': readCargoVersion('src-tauri/Cargo.toml'),
}

const unique = new Set(Object.values(versions))
for (const [file, version] of Object.entries(versions)) {
  console.log(`[版本同步] ${file}: ${version}`)
}

if (unique.size === 1) {
  console.log('[版本同步] OK：三处版本号一致')
} else {
  console.error('[版本同步] 失败：三处版本号不一致，请运行 `npm run tauri version <x.y.z>` 统一')
  process.exit(1)
}
