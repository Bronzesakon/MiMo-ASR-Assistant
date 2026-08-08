<script setup lang="ts">
import { computed } from 'vue'
import SingleChoiceDialog from './SingleChoiceDialog.vue'
import { usePromptsStore } from '../stores/prompts'

const store = usePromptsStore()

defineProps<{
  visible: boolean
}>()

const emit = defineEmits<{
  (e: 'select', id: string): void
  (e: 'close'): void
}>()

// 按 prompts 数组顺序仅显示标题，最多 8 条（F-06.6.3）；
// 最近项 = selected_prompt_id，仅背景高亮（highlight='background'）
const options = computed(() =>
  store.prompts.map(p => ({ value: p.id, label: p.title }))
)
</script>

<template>
  <SingleChoiceDialog
    :visible="visible"
    title="选择规整提示词（单选）"
    :options="options"
    :active-value="store.selectedPromptId"
    highlight="background"
    @select="(v: string) => emit('select', v)"
    @close="emit('close')"
  />
</template>
