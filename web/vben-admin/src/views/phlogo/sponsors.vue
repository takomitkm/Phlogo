<script setup lang="ts">
/**
 * Sponsors (B3 gap #2): CRUD for sponsor labels injected into node names.
 * Match semantics (upstream parity): priority ASC, later match overrides,
 * no match -> default_sponsor_text (currently '').
 * The filter is a single simple condition built here into a Filter-AST leaf
 * (the server whitelist validates it); power users can pass full ASTs via API.
 */
import { onMounted, reactive, ref } from 'vue';

import { ElMessage, ElMessageBox } from 'element-plus';

import {
  createSponsor,
  deleteSponsor,
  listSponsors,
  updateSponsor,
  type SponsorUpsert,
  type SponsorView,
} from '#/api/phlogo';

const sponsors = ref<SponsorView[]>([]);
const total = ref(0);
const page = ref(1);
const loading = ref(false);

const dialog = reactive<{
  visible: boolean;
  id: string | null;
  form: SponsorUpsert;
  cond: { field: 'region' | 'protocol' | 'server' | 'score' | 'health'; operator: string; value: string };
}>({
  visible: false,
  id: null,
  form: { name: '', text: '', filter_ast: null, priority: 0 },
  cond: { field: 'protocol', operator: 'eq', value: '' },
});

async function refresh() {
  loading.value = true;
  try {
    const resp = await listSponsors(page.value);
    sponsors.value = resp.data;
    total.value = resp.pagination.total;
  } catch (e) {
    ElMessage.error(`load sponsors failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
}

function conditionAst() {
  const { field, operator, value } = dialog.cond;
  if (!value.trim()) return null; // no condition = match everything
  if (field === 'score') {
    const n = Number(value);
    if (Number.isNaN(n)) throw new Error('score value must be a number');
    return { field, operator, value: n };
  }
  return { field, operator, value };
}

function openCreate() {
  dialog.id = null;
  dialog.form = { name: '', text: '', filter_ast: null, priority: 0 };
  dialog.cond = { field: 'protocol', operator: 'eq', value: '' };
  dialog.visible = true;
}

function openEdit(s: SponsorView) {
  dialog.id = s.id;
  dialog.form = { name: s.name, text: s.text, filter_ast: s.filter_ast, priority: s.priority };
  const leaf = (s.filter_ast ?? {}) as { field?: string; operator?: string; value?: unknown };
  dialog.cond = {
    field: (leaf.field as SponsorUpsert extends never ? never : 'region' | 'protocol' | 'server' | 'score' | 'health') ?? 'protocol',
    operator: leaf.operator ?? 'eq',
    value: leaf.value === null || leaf.value === undefined ? '' : String(leaf.value),
  };
  dialog.visible = true;
}

async function onSave() {
  try {
    dialog.form.filter_ast = conditionAst();
  } catch (e) {
    ElMessage.error(String(e).replace('Error: ', ''));
    return;
  }
  try {
    if (dialog.id) {
      await updateSponsor(dialog.id, dialog.form);
      ElMessage.success('saved');
    } else {
      await createSponsor(dialog.form);
      ElMessage.success('created');
    }
    dialog.visible = false;
    await refresh();
  } catch (e) {
    ElMessage.error(`save failed: ${String(e)}`);
  }
}

async function onDelete(s: SponsorView) {
  try {
    await ElMessageBox.confirm(`Delete sponsor "${s.name}"?`, 'Confirm', { type: 'warning' });
    await deleteSponsor(s.id);
    ElMessage.success('deleted');
    await refresh();
  } catch {
    /* cancelled */
  }
}

onMounted(refresh);
</script>

<template>
  <div class="p-4" v-loading="loading">
    <div class="flex items-center gap-2">
      <el-button type="primary" @click="openCreate">New Sponsor</el-button>
      <span class="text-muted-foreground text-sm">
        Priority ascending; later match overrides; no match uses the global default text.
      </span>
    </div>

    <el-table :data="sponsors" class="mt-3" border>
      <el-table-column prop="priority" label="Priority" width="90" sortable />
      <el-table-column prop="name" label="Name" min-width="140" />
      <el-table-column prop="text" label="Injected text" min-width="180" show-overflow-tooltip />
      <el-table-column label="Condition (AST)" min-width="240">
        <template #default="{ row }">
          <code class="text-xs">{{ JSON.stringify(row.filter_ast) }}</code>
        </template>
      </el-table-column>
      <el-table-column label="Actions" width="170">
        <template #default="{ row }">
          <el-button size="small" @click="openEdit(row)">Edit</el-button>
          <el-button size="small" type="danger" @click="onDelete(row)">Delete</el-button>
        </template>
      </el-table-column>
    </el-table>

    <el-pagination
      v-model:current-page="page"
      class="mt-3"
      layout="prev, pager, next"
      :total="total"
      :page-size="50"
      @current-change="refresh"
    />

    <el-dialog v-model="dialog.visible" :title="dialog.id ? 'Edit Sponsor' : 'New Sponsor'" width="560">
      <el-form label-width="120">
        <el-form-item label="Name"><el-input v-model="dialog.form.name" /></el-form-item>
        <el-form-item label="Injected text">
          <el-input v-model="dialog.form.text" placeholder="e.g. Premium" />
        </el-form-item>
        <el-form-item label="Priority">
          <el-input-number v-model="dialog.form.priority" :min="0" :max="10000" />
          <span class="ml-2 text-muted-foreground text-xs">lower evaluated first, later match wins</span>
        </el-form-item>
        <el-form-item label="Field">
          <el-select v-model="dialog.cond.field">
            <el-option value="protocol" label="protocol" />
            <el-option value="region" label="region" />
            <el-option value="server" label="server" />
            <el-option value="score" label="score" />
            <el-option value="health" label="health" />
          </el-select>
        </el-form-item>
        <el-form-item label="Operator">
          <el-select v-model="dialog.cond.operator">
            <el-option value="eq" label="equals" />
            <el-option value="ne" label="not equals" />
            <el-option value="contains" label="contains" />
            <el-option value="gte" label=">=" />
            <el-option value="lte" label="<=" />
          </el-select>
        </el-form-item>
        <el-form-item label="Value">
          <el-input v-model="dialog.cond.value" placeholder="empty = match every node" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialog.visible = false">Cancel</el-button>
        <el-button type="primary" @click="onSave">Save</el-button>
      </template>
    </el-dialog>
  </div>
</template>
