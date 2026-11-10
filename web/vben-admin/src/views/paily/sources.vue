<script setup lang="ts">
/**
 * Sources page (p6-02): list / create (dedup) / delete / ignore-dead.
 * Endpoint mapping: SourceList / SourceCreate / SourceDelete / SourceIgnoreDead.
 */
import { onMounted, reactive, ref } from 'vue';

import { ElMessage } from 'element-plus';

import {
  createSource,
  deleteSource,
  ignoreDead,
  listSources,
  type SourceView,
} from '#/api/paily';

const rows = ref<SourceView[]>([]);
const loading = ref(false);
const dialog = reactive({ visible: false, type: 'subscribe', content: '' });

async function refresh() {
  loading.value = true;
  try {
    rows.value = await listSources();
  } catch (e) {
    ElMessage.error(`load sources failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
}

async function onCreate() {
  try {
    const res = await createSource({ type: dialog.type, content: dialog.content });
    if (res.status === 201) {
      ElMessage.success('created');
    } else if (res.status === 200) {
      ElMessage.warning('already existed (dedup)');
    } else {
      ElMessage.error(`unexpected status ${res.status}`);
      return;
    }
    dialog.visible = false;
    dialog.content = '';
    await refresh();
  } catch (e) {
    ElMessage.error(`create failed: ${String(e)}`);
  }
}

async function onDelete(row: SourceView) {
  try {
    const res = await deleteSource(row.id);
    if (res.status === 204) ElMessage.success('deleted');
    else ElMessage.error(`unexpected status ${res.status}`);
    await refresh();
  } catch (e) {
    ElMessage.error(`delete failed: ${String(e)}`);
  }
}

async function onIgnoreDead(row: SourceView) {
  try {
    await ignoreDead(row.id);
    ElMessage.success('ignore-dead set');
    await refresh();
  } catch (e) {
    ElMessage.error(`ignore-dead failed: ${String(e)}`);
  }
}

onMounted(refresh);
</script>

<template>
  <div class="p-4">
    <el-button type="primary" @click="dialog.visible = true">New Source</el-button>
    <el-table v-loading="loading" :data="rows" class="mt-3" border>
      <el-table-column prop="type" label="Type" width="110" />
      <el-table-column prop="identifier" label="Identifier" width="160" />
      <el-table-column prop="content" label="Content" show-overflow-tooltip />
      <el-table-column prop="status" label="Status" width="100" />
      <el-table-column prop="fetch_policy" label="Policy" width="110" />
      <el-table-column label="Actions" width="230">
        <template #default="{ row }">
          <el-button size="small" @click="onIgnoreDead(row)">Ignore-Dead</el-button>
          <el-button size="small" type="danger" @click="onDelete(row)">Delete</el-button>
        </template>
      </el-table-column>
    </el-table>

    <el-dialog v-model="dialog.visible" title="New Source" width="560">
      <el-form label-width="90">
        <el-form-item label="Type">
          <el-select v-model="dialog.type">
            <el-option value="subscribe" label="subscribe" />
            <el-option value="node" label="node" />
          </el-select>
        </el-form-item>
        <el-form-item label="Content">
          <el-input v-model="dialog.content" type="textarea" :rows="3" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialog.visible = false">Cancel</el-button>
        <el-button type="primary" @click="onCreate">Create</el-button>
      </template>
    </el-dialog>
  </div>
</template>
