<script setup lang="ts">
/**
 * Dashboard (p6-02): stats cards + region/protocol distribution from NodeList.
 * Endpoints: StatsGet / CheckerList / NodeList.
 */
import { computed, onMounted, ref } from 'vue';

import { ElMessage } from 'element-plus';

import { getCheckers, getStats, listNodes, type NodeView, type Stats } from '#/api/paily';

const stats = ref<Stats | null>(null);
const checkers = ref<string[]>([]);
const nodes = ref<NodeView[]>([]);
const loading = ref(false);

const byRegion = computed(() => groupCount(nodes.value.map((n) => regionOf(n.server))));
const byProtocol = computed(() => groupCount(nodes.value.map((n) => n.protocol)));

function regionOf(server: string): string {
  // Servers look like host names; use the registrable suffix as a coarse region hint.
  const parts = server.split('.');
  return parts.length >= 2 ? parts[parts.length - 2].toUpperCase() : 'UN';
}

function groupCount(values: string[]): { key: string; count: number }[] {
  const m = new Map<string, number>();
  for (const v of values) m.set(v, (m.get(v) ?? 0) + 1);
  return [...m.entries()]
    .map(([key, count]) => ({ key, count }))
    .sort((a, b) => b.count - a.count)
    .slice(0, 10);
}

onMounted(async () => {
  loading.value = true;
  try {
    [stats.value, checkers.value, nodes.value] = await Promise.all([
      getStats(),
      getCheckers(),
      listNodes(),
    ]);
  } catch (e) {
    ElMessage.error(`load dashboard failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
});
</script>

<template>
  <div class="p-4" v-loading="loading">
    <el-row :gutter="12">
      <el-col :span="6"><el-card><el-statistic title="Total Nodes" :value="stats?.total_nodes ?? 0" /></el-card></el-col>
      <el-col :span="6"><el-card><el-statistic title="Alive Nodes" :value="stats?.alive_nodes ?? 0" /></el-card></el-col>
      <el-col :span="6"><el-card><el-statistic title="Total Sources" :value="stats?.total_sources ?? 0" /></el-card></el-col>
      <el-col :span="6"><el-card><el-statistic title="Dead Sources" :value="stats?.dead_sources ?? 0" /></el-card></el-col>
    </el-row>

    <el-row :gutter="12" class="mt-3">
      <el-col :span="12">
        <el-card header="Regions (top 10)">
          <el-table :data="byRegion" size="small">
            <el-table-column prop="key" label="Region" />
            <el-table-column prop="count" label="Count" width="100" />
          </el-table>
        </el-card>
      </el-col>
      <el-col :span="12">
        <el-card header="Protocols">
          <el-table :data="byProtocol" size="small">
            <el-table-column prop="key" label="Protocol" />
            <el-table-column prop="count" label="Count" width="100" />
          </el-table>
        </el-card>
      </el-col>
    </el-row>

    <el-card header="Checker tags" class="mt-3">
      <el-tag v-for="c in checkers" :key="c" class="mr-2">{{ c }}</el-tag>
      <span v-if="checkers.length === 0" class="text-gray-400">no checker tags yet</span>
    </el-card>
  </div>
</template>
