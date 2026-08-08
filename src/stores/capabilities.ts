// 能力 store（P8）：启动时加载后端 get_capabilities，作为 provider 元数据、
// 切片选项与导入/导出限制的单一来源，避免前端散落魔法常量。
// 前端限制仅用于即时交互，后端仍执行最终校验。
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { AppCapabilities } from '../types'

export const useCapabilitiesStore = defineStore('capabilities', () => {
  const caps = ref<AppCapabilities | null>(null)
  const loading = ref(false)

  // 同一会话只做一次正常加载，失败可显式重试
  let loadedOnce = false

  async function ensureLoaded(force = false): Promise<boolean> {
    if (caps.value && !force) return true
    if (loadedOnce && !force) return caps.value !== null
    if (loading.value) return caps.value !== null
    loading.value = true
    try {
      caps.value = await invoke<AppCapabilities>('get_capabilities')
      loadedOnce = true
      return true
    } catch (e) {
      console.error('[能力] 加载失败:', e)
      return false
    } finally {
      loading.value = false
    }
  }

  return { caps, loading, ensureLoaded }
})