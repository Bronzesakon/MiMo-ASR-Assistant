// UI store（清单 F-04）：设置抽屉开关、Toast、全局转写进行中标记。
import { defineStore } from 'pinia'
import { ref } from 'vue'

export type ToastType = 'info' | 'error' | 'success'

export const useUiStore = defineStore('ui', () => {
  const isSettingsOpen = ref(false)

  // 是否有文件正在转写/处理（由事件层与文件动作维护）
  const isTranscribing = ref(false)

  const toastMessage = ref('')
  const toastType = ref<ToastType>('info')
  const showToast = ref(false)

  // 连续调用 showToastMessage 时先清掉上一个定时器，避免 toast 提前消失（清单 F-04）
  let toastTimer: ReturnType<typeof setTimeout> | null = null

  function showToastMessage(message: string, type: ToastType = 'info') {
    toastMessage.value = message
    toastType.value = type
    showToast.value = true
    if (toastTimer !== null) clearTimeout(toastTimer)
    toastTimer = setTimeout(() => {
      showToast.value = false
      toastTimer = null
    }, 3000)
  }

  return {
    isSettingsOpen, isTranscribing,
    toastMessage, toastType, showToast, showToastMessage,
  }
})
