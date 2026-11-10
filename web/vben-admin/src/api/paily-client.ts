/**
 * Base API client (p6-01): base URL + JWT/service bearer injection + 401 handling.
 * Generated types come from `pnpm openapi:gen` (openapi-typescript) into
 * `src/api/generated/schema.d.ts`; pages use `openapi-fetch`'s typed client.
 */
import createClient from 'openapi-fetch';
import type { paths } from './generated/schema.d';

const BASE_URL = (import.meta.env?.VITE_API_BASE_URL as string | undefined) ?? '/api';

let accessToken: string | null = null;

export function setAuthToken(token: string | null): void {
  accessToken = token;
}

export const client = createClient<paths>({ baseUrl: BASE_URL });

export async function apiFetch<T>(
  input: string,
  init?: RequestInit & { json?: unknown },
): Promise<T> {
  const headers = new Headers(init?.headers);
  if (accessToken) headers.set('Authorization', `Bearer ${accessToken}`);
  if (init?.json !== undefined) headers.set('content-type', 'application/json');
  const res = await fetch(`${BASE_URL}${input}`, {
    ...init,
    headers,
    body: init?.json !== undefined ? JSON.stringify(init.json) : init?.body,
  });
  if (res.status === 401) {
    // Auth expiry: surface to the app shell (login redirect handled in stores).
    setAuthToken(null);
    throw new Error('unauthorized');
  }
  if (!res.ok) throw new Error(`api ${res.status}: ${await res.text()}`);
  return (await res.json()) as T;
}
