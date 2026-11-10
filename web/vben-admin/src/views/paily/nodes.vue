<script setup lang="ts">
/**
 * Nodes page (p6-02): list + delete.
 * Endpoint mapping: NodeList / NodeDelete.
 */
import { onMounted, ref } from 'vue';

import { ElMessage } from 'element-plus';

import { deleteNode, listNodes, type NodeView } from '#/api/paily';

const rows = ref<NodeView[]>([]);
const loading = ref(false);

async function refresh() {
  loading.value = true;
  try {
    rows.value = await listNodes();
  } catch (e) {
    ElMessage.error(`load nodes failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
}

async function onDelete(row: NodeView) {
  try {
    const res = await deleteNode(row.id);
    if (res.status === 204) ElMessage.success('deleted');
    else ElMessage.error(`unexpected status ${res.status}`);
    await refresh();
  } catch (e) {
    ElMessage.error(`delete failed: ${String(e)}`);
  }
}

onMounted(refresh);
</script>

<template>
  <div class="p-4">
    <el-table v-loading="loading" :data="rows" border>
      <el-table-column prop="protocol" label="Protocol" width="110" />
      <el-table-column prop="server" label="Server" min-width="180" />
      <el-table-column prop="port" label="Port" width="90" />
      <el-table-column prop="identity_hash" label="Identity" width="150" show-overflow-tooltip />
      <el-table-column prop="fingerprint_version" label="FP" width="70" />
      <el-table-column label="Actions" width="110">
        <template #default="{ row }">
          <el-button size="small" type="danger" @click="onDelete(row)">Delete</el-button>
        </template>
      </el-table-column>
    </el-table>
  </div>
</template>
