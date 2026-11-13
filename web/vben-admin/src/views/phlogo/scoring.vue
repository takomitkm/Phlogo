<script setup lang="ts">
/**
 * Scoring page (p6-02): view + edit score_* / grade_* config keys.
 * Endpoint mapping: ConfigGet / ConfigUpdate.
 */
import { onMounted, reactive, ref } from 'vue';

import { ElMessage } from 'element-plus';

import { getConfig, putConfig } from '#/api/phlogo';

const EDITABLE_KEYS = [
  'score_decay_alpha',
  'score_weight_latency',
  'score_weight_stability',
  'score_weight_speed',
  'score_latency_curve_k',
  'score_jitter_linear_threshold_ms',
  'grade_a_min',
  'grade_b_min',
  'grade_c_min',
  'max_distribute',
] as const;

const rows = reactive<Record<string, string>>({});
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    const cfg = await getConfig();
    for (const k of EDITABLE_KEYS) {
      rows[k] = cfg[k] ?? '';
    }
  } catch (e) {
    ElMessage.error(`load config failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
}

async function onSave() {
  try {
    const body: Record<string, string> = {};
    for (const k of EDITABLE_KEYS) body[k] = rows[k] ?? '';
    await putConfig(body);
    ElMessage.success('saved');
    await refresh();
  } catch (e) {
    ElMessage.error(`save failed: ${String(e)}`);
  }
}

onMounted(refresh);
</script>

<template>
  <div class="p-4">
    <el-form v-loading="loading" label-width="300" style="max-width: 640px">
      <el-form-item v-for="k in EDITABLE_KEYS" :key="k" :label="k">
        <el-input v-model="rows[k]" />
      </el-form-item>
      <el-form-item>
        <el-button type="primary" @click="onSave">Save</el-button>
      </el-form-item>
    </el-form>
  </div>
</template>
