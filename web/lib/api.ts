import type {
  AccountLookup, Call, CallWithMedia, CreatedCall, Device, DeviceUpdate, Invitation, Me, Meta, Page, RecentPerson, Recording,
  Settings, SettingsPatch, TranscriptPage, Voicemail,
} from './types';

// The web app talks to its own origin: Caddy proxies /v1 in production and the Next dev server rewrites it locally.
// The session is the HttpOnly ring_session cookie, so every request sends credentials, and every mutation carries
// X-Ring-Client: web, which the server requires as its CSRF check.

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly hint: string | null;
  readonly retryable: boolean;
  readonly details: Record<string, unknown>;

  constructor(status: number, code: string, message: string, hint: string | null = null, retryable = false, details: Record<string, unknown> = {}) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.hint = hint;
    this.retryable = retryable;
    this.details = details;
  }
}

export const isUnauthenticated = (error: unknown) =>
  error instanceof ApiError && (error.status === 401 || error.code === 'unauthenticated' || error.code === 'session_expired');

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

interface RequestOptions {
  body?: unknown;
  /** Reuse one key for every attempt of the same action, so a retried request returns the original result. */
  idempotencyKey?: string;
  signal?: AbortSignal;
}

// crypto.randomUUID exists only in secure contexts; getRandomValues works everywhere, so a page opened over plain
// http on a LAN address still renders.
export function newIdempotencyKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `web-${Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('')}`;
}

async function request<T>(method: Method, path: string, { body, idempotencyKey, signal }: RequestOptions = {}): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  if (method !== 'GET') headers['X-Ring-Client'] = 'web';
  if (body !== undefined) headers['Content-Type'] = 'application/json';
  if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey;
  let response: Response;
  try {
    response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'include',
      cache: 'no-store',
      signal,
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    throw new ApiError(0, 'network', 'Ring could not be reached. Check your connection.', null, true);
  }
  if (response.status === 204) return undefined as T;
  const text = await response.text();
  let parsed: unknown = undefined;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = undefined;
    }
  }
  if (!response.ok) {
    const error = (parsed as { error?: { code?: string; message?: string; hint?: string; retryable?: boolean; details?: Record<string, unknown> } } | undefined)?.error;
    throw new ApiError(
      response.status,
      error?.code ?? (response.status === 401 ? 'unauthenticated' : 'http_error'),
      error?.message ?? `Ring answered with ${response.status}.`,
      error?.hint ?? null,
      error?.retryable ?? response.status >= 500,
      error?.details ?? {},
    );
  }
  return parsed as T;
}

const enc = encodeURIComponent;

function query(params: Record<string, string | number | boolean | null | undefined>) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined && value !== null && value !== '') search.set(key, String(value));
  const text = search.toString();
  return text ? `?${text}` : '';
}

export const api = {
  meta: () => request<Meta>('GET', '/v1/meta'),
  me: () => request<Me>('GET', '/v1/me'),
  signOut: () => request<void>('DELETE', '/v1/session'),

  updateCurrentDevice: (update: DeviceUpdate) => request<Device>('PUT', '/v1/devices/current', { body: update }),
  devices: () => request<{ items: Device[] }>('GET', '/v1/devices'),
  patchDevice: (deviceId: string, patch: { name?: string; ring_enabled?: boolean }) => request<Device>('PATCH', `/v1/devices/${enc(deviceId)}`, { body: patch }),
  revokeDevice: (deviceId: string) => request<void>('DELETE', `/v1/devices/${enc(deviceId)}`),

  lookup: (ref: string, signal?: AbortSignal) => request<AccountLookup>('GET', `/v1/accounts/${enc(ref)}`, { signal }),
  recentPeople: (q = '', limit = 20, signal?: AbortSignal) => request<{ items: RecentPerson[] }>('GET', `/v1/people/recent${query({ q, limit })}`, { signal }),

  calls: (params: { state?: string; direction?: string; with?: string; limit?: number; cursor?: string | null } = {}) =>
    request<Page<Call>>('GET', `/v1/calls${query(params)}`),
  call: (ringid: string) => request<Call>('GET', `/v1/calls/${enc(ringid)}`),
  createCall: (to: string, idempotencyKey = newIdempotencyKey()) => request<CreatedCall>('POST', '/v1/calls', { body: { to }, idempotencyKey }),
  accept: (ringid: string, invitationId?: string) => request<CallWithMedia>('POST', `/v1/calls/${enc(ringid)}/accept`, { body: { invitation_id: invitationId } }),
  decline: (ringid: string, invitationId: string | undefined, reason?: string) =>
    request<{ call: Call }>('POST', `/v1/calls/${enc(ringid)}/decline`, { body: { invitation_id: invitationId, reason: reason || undefined } }),
  silence: (ringid: string, invitationId?: string) => request<{ call: Call }>('POST', `/v1/calls/${enc(ringid)}/silence`, { body: { invitation_id: invitationId } }),
  leave: (ringid: string) => request<{ call: Call }>('POST', `/v1/calls/${enc(ringid)}/leave`, { body: {} }),
  invite: (ringid: string, to: string, idempotencyKey = newIdempotencyKey()) =>
    request<{ call: Call; invitation: Invitation }>('POST', `/v1/calls/${enc(ringid)}/invite`, { body: { to }, idempotencyKey }),
  media: (ringid: string) => request<{ call: Call; media: CallWithMedia['media'] }>('POST', `/v1/calls/${enc(ringid)}/media`, { body: {} }),
  transcript: (ringid: string, afterSeq = 0, limit = 200) => request<TranscriptPage>('GET', `/v1/calls/${enc(ringid)}/transcript${query({ after_seq: afterSeq, limit })}`),
  recording: (ringid: string) => request<Recording>('GET', `/v1/calls/${enc(ringid)}/recording`),

  voicemails: (params: { unread?: boolean; limit?: number; cursor?: string | null } = {}) =>
    request<Page<Voicemail>>('GET', `/v1/voicemails${query({ unread: params.unread ? true : undefined, limit: params.limit, cursor: params.cursor })}`),
  voicemail: (id: string) => request<Voicemail>('GET', `/v1/voicemails/${enc(id)}`),
  markVoicemail: (id: string, read: boolean) => request<Voicemail>('PATCH', `/v1/voicemails/${enc(id)}`, { body: { read } }),
  deleteVoicemail: (id: string) => request<void>('DELETE', `/v1/voicemails/${enc(id)}`),

  settings: () => request<Settings>('GET', '/v1/settings'),
  patchSettings: (patch: SettingsPatch) => request<Settings>('PATCH', '/v1/settings', { body: patch }),
};

/** Reads every transcript page after `afterSeq`. */
export async function fullTranscript(ringid: string, afterSeq = 0) {
  const entries: TranscriptPage['items'] = [];
  let cursor = afterSeq;
  for (let page = 0; page < 50; page++) {
    const result = await api.transcript(ringid, cursor, 200);
    entries.push(...result.items);
    if (result.items.length < 200 || result.next_after_seq <= cursor) break;
    cursor = result.next_after_seq;
  }
  return entries;
}

export function errorMessage(error: unknown) {
  if (error instanceof ApiError) return error.hint ? `${error.message} ${error.hint}` : error.message;
  if (error instanceof Error) return error.message;
  return 'Something went wrong.';
}
