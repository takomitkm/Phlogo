<script setup lang="ts">
/**
 * Subscriptions (p6-02): profile CRUD + token issue (plaintext shown once) / revoke.
 * Endpoints: Subscriptions CRUD / Tokens issue/list/revoke.
 */
import { onMounted, reactive, ref } from 'vue';

import { ElMessage, ElMessageBox } from 'element-plus';

import {
  createSubscription,
  deleteSubscription,
  issueToken,
  listSubscriptions,
  listTokens,
  revokeToken,
  updateSubscription,
  type ProfileUpsert,
  type ProfileView,
  type TokenRow,
} from '#/api/phlogo';

const profiles = ref<ProfileView[]>([]);
const loading = ref(false);
const dialog = reactive<{ visible: boolean; id: string | null; form: ProfileUpsert }>({
  visible: false,
  id: null,
  form: { name: '', preset: 'common', health: 'alive', min_score: 0, limit_count: 200, enabled: true },
});
const tokens = reactive<{ profileId: string | null; rows: TokenRow[] }>({ profileId: null, rows: [] });

async function refresh() {
  loading.value = true;
  try {
    profiles.value = await listSubscriptions();
    if (tokens.profileId) await refreshTokens(tokens.profileId);
  } catch (e) {
    ElMessage.error(`load subscriptions failed: ${String(e)}`);
  } finally {
    loading.value = false;
  }
}

function openCreate() {
  dialog.id = null;
  dialog.form = { name: '', preset: 'common', health: 'alive', min_score: 0, limit_count: 200, enabled: true };
  dialog.visible = true;
}

function openEdit(p: ProfileView) {
  dialog.id = p.id;
  dialog.form = {
    name: p.name,
    preset: p.preset,
    health: p.health,
    min_score: p.min_score,
    limit_count: p.limit_count,
    enabled: p.enabled,
  };
  dialog.visible = true;
}

async function onSave() {
  try {
    if (dialog.id) {
      await updateSubscription(dialog.id, dialog.form);
      ElMessage.success('saved (version bumped, cache invalidated)');
    } else {
      await createSubscription(dialog.form);
      ElMessage.success('created');
    }
    dialog.visible = false;
    await refresh();
  } catch (e) {
    ElMessage.error(`save failed: ${String(e)}`);
  }
}

async function onDelete(p: ProfileView) {
  try {
    await ElMessageBox.confirm(`Delete profile "${p.name}" and its tokens?`, 'Confirm', { type: 'warning' });
    await deleteSubscription(p.id);
    ElMessage.success('deleted');
    await refresh();
  } catch {
    /* cancelled */
  }
}

async function onIssue(p: ProfileView) {
  try {
    const { token } = await issueToken(p.id);
    await ElMessageBox.alert(
      token,
      'New token (shown only once — copy now)',
      { confirmButtonText: 'Copied' },
    );
    tokens.profileId = p.id;
    await refreshTokens(p.id);
  } catch (e) {
    ElMessage.error(`issue failed: ${String(e)}`);
  }
}

async function refreshTokens(profileId: string) {
  tokens.profileId = profileId;
  tokens.rows = await listTokens(profileId);
}

async function onRevoke(t: TokenRow) {
  try {
    await revokeToken(t.id);
    ElMessage.success('revoked (immediate 401 on /sub)');
    if (tokens.profileId) await refreshTokens(tokens.profileId);
  } catch (e) {
    ElMessage.error(`revoke failed: ${String(e)}`);
  }
}

onMounted(refresh);
</script>

<template>
  <div class="p-4" v-loading="loading">
    <el-button type="primary" @click="openCreate">New Profile</el-button>

    <el-table :data="profiles" class="mt-3" border>
      <el-table-column prop="name" label="Name" min-width="140" />
      <el-table-column prop="preset" label="Preset" width="110" />
      <el-table-column prop="health" label="Health" width="100" />
      <el-table-column prop="min_score" label="Min score" width="100" />
      <el-table-column prop="limit_count" label="Limit" width="80" />
      <el-table-column label="Enabled" width="90">
        <template #default="{ row }">
          <el-tag :type="row.enabled ? 'success' : 'info'" size="small">{{ row.enabled }}</el-tag>
        </template>
      </el-table-column>
      <el-table-column prop="version" label="Ver" width="70" />
      <el-table-column label="Actions" width="270">
        <template #default="{ row }">
          <el-button size="small" @click="onIssue(row)">Issue token</el-button>
          <el-button size="small" @click="openEdit(row)">Edit</el-button>
          <el-button size="small" type="danger" @click="onDelete(row)">Delete</el-button>
        </template>
      </el-table-column>
    </el-table>

    <el-card v-if="tokens.profileId" header="Tokens for selected profile (hashes stored server-side)" class="mt-3">
      <el-table :data="tokens.rows" size="small" border>
        <el-table-column label="Token ID" width="300">
          <template #default="{ row }">{{ row.id }}</template>
        </el-table-column>
        <el-table-column label="Status" width="110">
          <template #default="{ row }">
            <el-tag :type="row.status === 'active' ? 'success' : 'danger'" size="small">{{ row.status }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="created_at" label="Created" show-overflow-tooltip />
        <el-table-column label="Actions" width="110">
          <template #default="{ row }">
            <el-button v-if="row.status === 'active'" size="small" type="danger" @click="onRevoke(row)">
              Revoke
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <el-dialog v-model="dialog.visible" :title="dialog.id ? 'Edit Profile' : 'New Profile'" width="560">
      <el-form label-width="110">
        <el-form-item label="Name"><el-input v-model="dialog.form.name" /></el-form-item>
        <el-form-item label="Preset">
          <el-select v-model="dialog.form.preset">
            <el-option value="common" label="common" />
            <el-option value="all" label="all" />
            <el-option value="other" label="other" />
          </el-select>
        </el-form-item>
        <el-form-item label="Min score"><el-input-number v-model="dialog.form.min_score" :min="0" :max="1" :step="0.05" /></el-form-item>
        <el-form-item label="Limit"><el-input-number v-model="dialog.form.limit_count" :min="1" :max="1000" /></el-form-item>
        <el-form-item label="Enabled"><el-switch v-model="dialog.form.enabled" /></el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dialog.visible = false">Cancel</el-button>
        <el-button type="primary" @click="onSave">Save</el-button>
      </template>
    </el-dialog>
  </div>
</template>
