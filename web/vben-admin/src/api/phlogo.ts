/**
 * phlogo API layer (p6-02 + contract-gap closures). Self-contained typed calls;
 * pages never touch the database or MiaoSpeed directly (hard constraint).
 */

function authHeaders(json = false): Headers {
  const headers = new Headers();
  if (json) headers.set('content-type', 'application/json');
  const token = (window as unknown as { __PHLOGO_TOKEN__?: string }).__PHLOGO_TOKEN__;
  if (token) headers.set('Authorization', `Bearer ${token}`);
  return headers;
}

async function expectOk(res: Response): Promise<void> {
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new Error(`api ${res.status}: ${body.slice(0, 200)}`);
  }
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: authHeaders() });
  await expectOk(res);
  return (await res.json()) as T;
}

async function send(url: string, method: string, body?: unknown): Promise<Response> {
  return fetch(url, {
    method,
    headers: authHeaders(body !== undefined),
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

// ---------- sources / nodes ----------

export interface SourceView {
  id: string;
  type: string;
  identifier: string;
  info: string;
  content: string;
  status: string;
  ignore_dead: boolean;
  fetch_policy: string;
}

export async function listSources(): Promise<SourceView[]> {
  return getJson<SourceView[]>('/api/v1/sources');
}

export async function createSource(
  body: Pick<SourceView, 'type' | 'content'> & Partial<SourceView>,
): Promise<Response> {
  return send('/api/v1/sources', 'POST', body);
}

export async function deleteSource(id: string): Promise<Response> {
  return send(`/api/v1/sources/${id}`, 'DELETE');
}

export async function ignoreDead(id: string): Promise<Response> {
  return send(`/api/v1/sources/${id}/ignore-dead`, 'POST');
}

export interface NodeView {
  id: string;
  identity_hash: string;
  fingerprint_version: number;
  protocol: string;
  server: string;
  port: number;
}

export async function listNodes(): Promise<NodeView[]> {
  return getJson<NodeView[]>('/api/v1/nodes');
}

export async function getNode(id: string): Promise<NodeView> {
  return getJson<NodeView>(`/api/v1/nodes/${id}`);
}

export async function deleteNode(id: string): Promise<Response> {
  return send(`/api/v1/nodes/${id}`, 'DELETE');
}

// ---------- config ----------

export interface ConfigMap {
  [key: string]: string;
}

export async function getConfig(): Promise<ConfigMap> {
  return getJson<ConfigMap>('/api/v1/config');
}

export async function putConfig(body: ConfigMap): Promise<void> {
  await send('/api/v1/config', 'PUT', { values: body });
}

// ---------- stats / checkers / logs (contract-gap closures) ----------

export interface Stats {
  total_nodes: number;
  alive_nodes: number;
  total_sources: number;
  dead_sources: number;
}

export async function getStats(): Promise<Stats> {
  return getJson<Stats>('/api/v1/stats');
}

export async function getCheckers(): Promise<string[]> {
  const r = await getJson<{ checkers: string[] }>('/api/v1/checkers');
  return r.checkers;
}

export interface CheckLogRow {
  node_id: string;
  latency_ms: number;
  checked_at: string;
  checker_tag: string;
}

export async function getCheckLogs(nodeId?: string): Promise<CheckLogRow[]> {
  const q = nodeId ? `?node_id=${encodeURIComponent(nodeId)}` : '';
  return getJson<CheckLogRow[]>(`/api/v1/logs/check${q}`);
}

export interface FetchLogRow {
  source_id: string;
  status: string;
  success: boolean;
  node_count: number;
  fetched_at: string;
}

export async function getFetchLogs(sourceId?: string): Promise<FetchLogRow[]> {
  const q = sourceId ? `?source_id=${encodeURIComponent(sourceId)}` : '';
  return getJson<FetchLogRow[]>(`/api/v1/logs/fetch${q}`);
}

export interface InitialCheckRow {
  latency_ms: number;
  checked_at: string;
  checker_tag: string;
  check_run_id: string;
}

export interface DeepCheckRow {
  avg_latency_ms?: number;
  jitter_ms?: number;
  speed_kbps?: number;
  region?: string;
  checked_at: string;
  checker_tag: string;
}

export async function getNodeInitialChecks(id: string): Promise<InitialCheckRow[]> {
  return getJson<InitialCheckRow[]>(`/api/v1/nodes/${id}/checks/initial`);
}

export async function getNodeDeepChecks(id: string): Promise<DeepCheckRow[]> {
  return getJson<DeepCheckRow[]>(`/api/v1/nodes/${id}/checks/deep`);
}

// ---------- subscriptions admin ----------

export interface ProfileView {
  id: string;
  name: string;
  filter_ast: unknown | null;
  region: string | null;
  health: string;
  min_score: number;
  limit_count: number;
  preset: string;
  naming_rule: string | null;
  enabled: boolean;
  version: number;
}

export interface ProfileUpsert {
  name: string;
  filter_ast?: unknown | null;
  region?: string | null;
  health?: string;
  min_score?: number;
  limit_count?: number;
  preset?: string;
  naming_rule?: string | null;
  enabled?: boolean;
}

export async function listSubscriptions(): Promise<ProfileView[]> {
  return getJson<ProfileView[]>('/api/v1/subscriptions');
}

export async function createSubscription(body: ProfileUpsert): Promise<{ id: string }> {
  const res = await send('/api/v1/subscriptions', 'POST', body);
  await expectOk(res);
  return (await res.json()) as { id: string };
}

export async function updateSubscription(id: string, body: ProfileUpsert): Promise<void> {
  await expectOk(await send(`/api/v1/subscriptions/${id}`, 'PUT', body));
}

export async function deleteSubscription(id: string): Promise<void> {
  await expectOk(await send(`/api/v1/subscriptions/${id}`, 'DELETE'));
}

export interface TokenRow {
  id: string;
  status: string;
  created_at: string;
  revoked_at: string | null;
}

export async function listTokens(profileId: string): Promise<TokenRow[]> {
  return getJson<TokenRow[]>(`/api/v1/subscriptions/${profileId}/tokens`);
}

/** Plaintext token is returned exactly once at issuance. */
export async function issueToken(profileId: string): Promise<{ id: string; token: string }> {
  const res = await send(`/api/v1/subscriptions/${profileId}/tokens`, 'POST');
  await expectOk(res);
  return (await res.json()) as { id: string; token: string };
}

export async function revokeToken(tokenId: string): Promise<void> {
  await expectOk(await send(`/api/v1/subscriptions/tokens/${tokenId}`, 'DELETE'));
}

// ---------- sponsors (B3 gap #2, restored) ----------

export interface SponsorView {
  id: string;
  name: string;
  text: string;
  filter_ast: unknown | null;
  priority: number;
}

export interface SponsorUpsert {
  name: string;
  text: string;
  filter_ast?: unknown | null;
  priority?: number;
}

export interface SponsorListResponse {
  data: SponsorView[];
  pagination: { page: number; limit: number; total: number; total_pages: number };
}

export async function listSponsors(page = 1, limit = 50): Promise<SponsorListResponse> {
  return getJson<SponsorListResponse>(`/api/v1/sponsors?page=${page}&limit=${limit}`);
}

export async function createSponsor(body: SponsorUpsert): Promise<SponsorView> {
  const res = await send('/api/v1/sponsors', 'POST', body);
  await expectOk(res);
  return (await res.json()) as SponsorView;
}

export async function updateSponsor(id: string, body: Partial<SponsorUpsert>): Promise<SponsorView> {
  const res = await send(`/api/v1/sponsors/${id}`, 'PATCH', body);
  await expectOk(res);
  return (await res.json()) as SponsorView;
}

export async function deleteSponsor(id: string): Promise<void> {
  await expectOk(await send(`/api/v1/sponsors/${id}`, 'DELETE'));
}

/** Build a Filter-AST condition from simple form fields (server whitelist enforces it). */
export function simpleCondition(
  field: 'region' | 'protocol' | 'server' | 'score' | 'health',
  operator: 'eq' | 'ne' | 'contains' | 'gte' | 'lte',
  value: string | number,
): unknown {
  return { field, operator, value };
}
