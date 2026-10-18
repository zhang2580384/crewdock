<script setup lang="ts">
import type { Project } from '../types';

defineProps<{
  projects: Project[];
  modelValue: string;
  disabled?: boolean;
}>();

defineEmits<{
  'update:modelValue': [value: string];
}>();
</script>

<template>
  <label class="field compact">
    <span>项目</span>
    <select :value="modelValue" :disabled="disabled" @change="$emit('update:modelValue', ($event.target as HTMLSelectElement).value)">
      <option value="" disabled>选择项目</option>
      <option v-for="project in projects" :key="project.id" :value="project.id">
        {{ project.name }}
      </option>
    </select>
  </label>
</template>
