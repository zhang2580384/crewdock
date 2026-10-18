<script setup lang="ts">
import { computed, nextTick, ref } from 'vue';

const props = defineProps<{
  modelValue: string;
  categories: string[];
  required?: boolean;
}>();
const emit = defineEmits<{ 'update:modelValue': [value: string] }>();
const creating = ref(!props.modelValue.trim());
const input = ref<HTMLInputElement | null>(null);
const options = computed(() => {
  const values = [...props.categories];
  const current = props.modelValue.trim();
  if (!creating.value && current && !values.includes(current)) values.push(current);
  return values;
});

async function selectCategory(event: Event) {
  const value = (event.target as HTMLSelectElement).value;
  creating.value = !value;
  emit('update:modelValue', value);
  if (creating.value) {
    await nextTick();
    input.value?.focus();
  }
}
</script>

<template>
  <div class="field">
    <label class="field">
      <span>分组</span>
      <select :value="creating ? '' : modelValue.trim()" @change="selectCategory">
        <option v-for="category in options" :key="category" :value="category">{{ category }}</option>
        <option value="">＋ 新建分组…</option>
      </select>
    </label>
    <input
      v-if="creating || !modelValue.trim()"
      ref="input"
      :value="modelValue"
      :required="required"
      aria-label="新分组名称"
      placeholder="输入新分组名称"
      @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
    />
  </div>
</template>
