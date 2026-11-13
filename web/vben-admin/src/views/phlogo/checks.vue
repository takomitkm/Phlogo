<script setup lang="ts">
/**
 * Checks page (p6-02): checker tags + recent initial-check log; latency -1 = unreachable.
 * Endpoints: CheckerList / LogCheckList.
 */
import { onMounted, ref } from 'vue';

import { ElMessage } from 'element-plus';

import { getCheckLogs, getCheckers, type CheckLogRow } from '#/api/phlogo';

const tags = ref<string[]>([]);
const rows = ref<CheckLogRow[]>([]);
const loading = ref(false);

function latencyText(v: number): string {
  return v === -1 ? 'unreachable' : `${v} ms`;
}

onMounted(async () => {
  loading.value = true;
  try {
    [tags.value, rows.value] = await Promise.all([getCheckers(), getCheckLogs()]);
  } catch (e) {
    ElMessage.error(`load checks failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="p-4" v-loading="loading">
    <el-card header="Checker tags">
      <el-tag v-for="t in tags" :key="t" class="mr-2">{{ t }}</el-tag>
      <span v-if="tags.length === 0" class="text-gray-400">none</span>
    </el-card>

    <el-card header="Recent initial checks (newest first)" class="mt-3">
      <el-table :data="rows" size="small" border>
        <el-table-column label="Node" width="300">
          <template #default="{ row }">{{ row.node_id.slice(0, 8) }}…</template>
        </el-table-column>
        <el-table-column label="Latency" width="140">
          <template #default="{ row }">
            <el-tag :type="row.latency_ms === -1 ? 'danger' : 'success'" size="small">
              {{ latencyText(row.latency_ms) }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="checker_tag" label="Tag" width="110" />
        <el-table-column prop="checked_at" label="Checked at" show-overflow-tooltip />
      </el-table>
    </el-card>
  </div>
</template>
