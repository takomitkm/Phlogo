/**
 * paily API layer (p6-02). Self-contained typed calls; pages never touch the
 * database or MiaoSpeed directly (hard constraint). Auth bearer is read from
 * the vben access store token via `window.__PAILY_TOKEN__` set by pages that
 * obtain one (service-token flow); unauthenticated calls simply omit it.
 */

function authHeaders(json = false): Headers {
  const headers = new Headers();
  if (json) headers.set('content-type', 'application/json');
  const token = (window as unknown as { __PAILY_TOKEN__?: string }).__PAILY_TOKEN__;
  if (token) headers.set('Authorization', `Bearer ${token}`);
  return headers;
}

async function expectOk(res: Response): Promise<void> {
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new Error(`api ${res.status}: ${body.slice(0, 200)}`);
  }
}

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

export interface NodeView {
  id: string;
  identity_hash: string;
  fingerprint_version: number;
  protocol: string;
  server: string;
  port: number;
}

export async function listSources(): Promise<SourceView[]> {
  const res = await fetch('/api/v1/sources', { headers: authHeaders() });
  await expectOk(res);
  return (await res.json()) as SourceView[];
}

export async function createSource(
  body: Pick<SourceView, 'type' | 'content'> & Partial<SourceView>,
): Promise<Response> {
  return fetch('/api/v1/sources', {
    method: 'POST',
    headers: authHeaders(true),
    body: JSON.stringify(body),
  });
}

export async function deleteSource(id: string): Promise<Response> {
  return fetch(`/api/v1/sources/${id}`, { method: 'DELETE', headers: authHeaders() });
}

export async function ignoreDead(id: string): Promise<Response> {
  return fetch(`/api/v1/sources/${id}/ignore-dead`, {
    method: 'POST',
    headers: authHeaders(),
  });
}

export async function listNodes(): Promise<NodeView[]> {
  const res = await fetch('/api/v1/nodes', { headers: authHeaders() });
  await expectOk(res);
  return (await res.json()) as NodeView[];
}

export async function deleteNode(id: string): Promise<Response> {
  return fetch(`/api/v1/nodes/${id}`, { method: 'DELETE', headers: authHeaders() });
}

export interface ConfigMap {
  [key: string]: string;
}

export async function getConfig(): Promise<ConfigMap> {
  const res = await fetch('/api/v1/config', { headers: authHeaders() });
  await expectOk(res);
  return (await res.json()) as ConfigMap;
}

export async function putConfig(body: ConfigMap): Promise<void> {
  const res = await fetch('/api/v1/config', {
    method: 'PUT',
    headers: authHeaders(true),
    body: JSON.stringify({ values: body }),
  });
  await expectOk(res);
}
