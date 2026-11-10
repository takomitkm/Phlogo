<script setup lang="ts">
/**
 * Node Detail (p6-02): identity summary + initial/deep check timelines per tag.
 * Endpoints: NodeGet / NodeGetInitialChecks / NodeGetDeepChecks.
 */
import { onMounted, ref } from 'vue';
import { useRoute } from 'vue-router';

import { ElMessage } from 'element-plus';

import {
  getNode,
  getNodeDeepChecks,
  getNodeInitialChecks,
  type DeepCheckRow,
  type InitialCheckRow,
  type NodeView,
} from '#/api/paily';

const node = ref<NodeView | null>(null);
const initial = ref<InitialCheckRow[]>([]);
const deep = ref<DeepCheckRow[]>([]);
const loading = ref(false);

const route = useRoute();
const nodeId = route.params.id as string;

onMounted(async () => {
  loading.value = true;
  try {
    [node.value, initial.value, deep.value] = await Promise.all([
      getNode(nodeId),
      getNodeInitialChecks(nodeId),
      getNodeDeepChecks(nodeId),
    ]);
  } catch (e) {
    ElMessage.error(`load node failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="p-4" v-loading="loading">
    <el-descriptions v-if="node" title="Node" border :column="2">
      <el-descriptions-item label="ID">{{ node.id }}</el-descriptions-item>
      <el-descriptions-item label="Protocol">{{ node.protocol }}</el-descriptions-item>
      <el-descriptions-item label="Server">{{ node.server }}</el-descriptions-item>
      <el-descriptions-item label="Port">{{ node.port }}</el-descriptions-item>
      <el-descriptions-item label="Identity hash" :span="2">{{ node.identity_hash }}</el-descriptions-item>
      <el-descriptions-item label="Fingerprint version">{{ node.fingerprint_version }}</el-descriptions-item>
    </el-descriptions>

    <el-card header="Initial checks (latency -1 = unreachable)" class="mt-3">
      <el-table :data="initial" size="small" border>
        <el-table-column label="Latency" width="140">
          <template #default="{ row }">
            <el-tag :type="row.latency_ms === -1 ? 'danger' : 'success'" size="small">
              {{ row.latency_ms === -1 ? 'unreachable' : `${row.latency_ms} ms` }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="checker_tag" label="Tag" width="110" />
        <el-table-column prop="checked_at" label="Checked at" show-overflow-tooltip />
      </el-table>
    </el-card>

    <el-card header="Deep checks" class="mt-3">
      <el-table :data="deep" size="small" border>
        <el-table-column label="Avg latency" width="120">
          <template #default="{ row }">{{ row.avg_latency_ms ?? '—' }} ms</template>
        </el-table-column>
        <el-table-column label="Jitter" width="110">
          <template #default="{ row }">{{ row.jitter_ms ?? '—' }} ms</template>
        </el-table-column>
        <el-table-column label="Speed" width="120">
          <template #default="{ row }">{{ row.speed_kbps ? `${row.speed_kbps} kbps` : 'untested' }}</template>
        </el-table-column>
        <el-table-column prop="region" label="Region" width="90" />
        <el-table-column prop="checker_tag" label="Tag" width="110" />
        <el-table-column prop="checked_at" label="Checked at" show-overflow-tooltip />
      </el-table>
    </el-card>
  </div>
</template>
