export type RingError = { code: string; message: string; next_action?: string; retryable?: boolean; details?: { organizations?: string[] } };
export type RingEvent = { type: string; data: any; seq?: number; event_id?: string };
export type Session = { actor: string; actor_id?: string; display_name?: string; org_id: string; device_id: string; session_token: string; expires_at?: string; realm?: string; url?: string };
export type ConnectSettings = { url: string; realm: string; org_id?: string; test_app_secret?: string };
export type Call = { ringid: string; caller: string; target: string; state: string; created_at: string; answered_at?: string; ended_at?: string; outcome?: string; recording_status?: string; participants: { actor: string; display_name?: string; device_id?: string; left_at?: string | null }[]; invitations: { invitation_id: string; inviter: string; target: string; state: string; reason?: string; expires_at: string }[] };
export const normalizeActor = (value: string) => value.trim().replace(/^@/, '').replace(/^((?:c|si):[^\s\[\]]+)\[[^\s\[\]]+\]$/, '$1');
export const isActor = (value: string) => /^(?:c|si):[^\s\[\]]+$/.test(normalizeActor(value));
export const displayActor = (value = '') => value.replace(/^(?:c|si):/, '').replace(/\[.*\]$/, '');
export function socketUrl(value: string) {
  const url = new URL(value);
  if (!['ws:', 'wss:'].includes(url.protocol) || url.username || url.password) throw new Error('Enter a WebSocket URL starting with ws:// or wss://.');
  if (url.protocol === 'ws:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) throw new Error('Use wss:// for remote servers. Plain WebSockets are allowed only on this device.');
  return url.toString();
}
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);
export function normalizeRealm(value: string) {
  const realm = value.trim().toLowerCase();
  if (realm === 'production' || realm === 'test' || /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(realm) && realm !== '00000000-0000-0000-0000-000000000000') return realm;
  throw new Error('Choose production, legacy test, or a valid Honeycomb environment UUID.');
}
function checkRealm(realm: string, acknowledged: unknown) {
  if (acknowledged === realm || acknowledged === undefined && ['production', 'test'].includes(realm)) return;
  throw Object.assign(new Error('The server did not confirm the selected environment. Sign in again after checking connection settings.'), { code: 'SESSION_REALM_MISMATCH' });
}
export function checkSessionContext(session: Session, settings: ConnectSettings) {
  if (session.realm !== normalizeRealm(settings.realm) || session.url !== socketUrl(settings.url) || settings.org_id && session.org_id !== settings.org_id) {
    throw Object.assign(new Error('The saved session belongs to different connection settings. Sign in again.'), { code: 'SESSION_CONTEXT_MISMATCH' });
  }
}
export function bindSession(session: Session, settings: ConnectSettings): Session {
  const realm = normalizeRealm(settings.realm);
  checkRealm(realm, session.realm);
  const bound = { ...session, realm, url: socketUrl(settings.url) };
  checkSessionContext(bound, settings);
  return bound;
}
export function sessionToRestore(saved: Session | null, native: Session | null): Session | null {
  if (!saved) return native;
  if (!saved.url && native?.url && native.realm && native.session_token === saved.session_token && native.device_id === saved.device_id && (!saved.realm || saved.realm === native.realm)) return native;
  return saved;
}

export function persistentStorage(persistent: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>, temporary: Pick<Storage, 'getItem' | 'removeItem'>) {
  return {
    read<T>(key: string, fallback: T): T {
      try {
        const value = persistent.getItem(key) || temporary.getItem(key);
        if (!value) return fallback;
        const parsed = JSON.parse(value);
        try { persistent.setItem(key, value); temporary.removeItem(key); } catch {}
        return parsed || fallback;
      } catch { return fallback; }
    },
    write(key: string, value: unknown) { persistent.setItem(key, JSON.stringify(value)); temporary.removeItem(key); },
    remove(key: string) { persistent.removeItem(key); temporary.removeItem(key); },
  };
}

export type CarbonLogin = { state: string; return_url: string; expires_at: number; connection: ConnectSettings };
export type CarbonExchange = { token: string; id: string; expected_actor_type: 'carbon'; expires_at: number; connection: ConnectSettings };
export function restoreCarbonExchange(saved: CarbonExchange | null, now = Date.now()): CarbonExchange | null {
  if (!saved || typeof saved.token !== 'string' || !saved.token.trim() || typeof saved.id !== 'string' || !saved.id || saved.expected_actor_type !== 'carbon' || typeof saved.expires_at !== 'number' || saved.expires_at <= now || saved.expires_at > now + 120000) return null;
  try {
    if (socketUrl(saved.connection.url) !== saved.connection.url || normalizeRealm(saved.connection.realm) !== saved.connection.realm) return null;
    return saved;
  } catch { return null; }
}
export function carbonLogin(iamLoginUrl: string, appId: string, returnUrl: string, connection: ConnectSettings, now = Date.now()) {
  const login = new URL(iamLoginUrl), callback = new URL(returnUrl);
  for (const url of [login, callback]) {
    if (url.username || url.password || !(url.protocol === 'https:' || url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) throw new Error('Carbon sign-in requires HTTPS or a localhost web address.');
  }
  if (!/^[a-z][a-z0-9_-]{0,63}$/.test(appId)) throw new Error('Ring returned an invalid IAM application ID.');
  callback.hash = '';
  for (const key of ['slt', 'ring_auth_state', 'error', 'error_description']) callback.searchParams.delete(key);
  const pending: CarbonLogin = { state: crypto.randomUUID(), return_url: callback.toString(), expires_at: now + 10 * 60 * 1000, connection: { ...connection, url: socketUrl(connection.url), realm: normalizeRealm(connection.realm) } };
  // IAM preserves redirect_uri query parameters; it does not echo a top-level state parameter.
  callback.searchParams.set('ring_auth_state', pending.state);
  login.search = new URLSearchParams({ app_id: appId, identity_kind: 'carbon', redirect_uri: callback.toString() }).toString();
  login.hash = '';
  return { url: login.toString(), pending };
}
export function carbonCallback(currentUrl: string, pending: CarbonLogin | null, now = Date.now()) {
  const url = new URL(currentUrl);
  if (!url.searchParams.has('slt') && !url.searchParams.has('ring_auth_state')) return null;
  const tokens = url.searchParams.getAll('slt'), states = url.searchParams.getAll('ring_auth_state');
  const denied = url.searchParams.has('error');
  for (const key of ['slt', 'ring_auth_state', 'error', 'error_description']) url.searchParams.delete(key);
  const cleanUrl = url.toString();
  const valid = pending && states.length === 1 && states[0] === pending.state && cleanUrl === pending.return_url && pending.expires_at > now && pending.expires_at <= now + 10 * 60 * 1000;
  if (!valid) return { cleanUrl, error: 'This IAM sign-in expired or was not started in this tab. Continue as Carbon again.' };
  if (denied || tokens.length !== 1 || !tokens[0].trim()) return { cleanUrl, error: 'IAM did not complete sign-in. Continue as Carbon again.' };
  return { cleanUrl, token: tokens[0], connection: pending.connection };
}
export function isExpiredSession(error: unknown) {
  const fault = error as RingError;
  return fault?.retryable !== true && ['AUTH_REQUIRED', 'IAM_AUTH_FAILED', 'IAM_TOKEN_INVALID', 'IAM_LOGIN_REQUIRED', 'SESSION_CONTEXT_MISMATCH', 'SESSION_REALM_MISMATCH'].includes(fault?.code);
}

export class RingSocket {
  private socket?: WebSocket;
  private pending = new Map<string, { resolve: (result: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  private listeners = new Set<(event: RingEvent) => void>();
  private retry?: ReturnType<typeof setTimeout>;
  private closed = false;
  private opening?: Promise<void>;
  private settings?: ConnectSettings;
  private helloReady = false;
  session?: Session;
  onStatus: (state: 'offline' | 'connecting' | 'connected' | 'reconnecting') => void = () => {};
  onExpired: () => void = () => {};
  onSession: (session: Session) => void = () => {};
  subscribe(listener: (event: RingEvent) => void) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  get ready() { return this.socket?.readyState === WebSocket.OPEN && this.helloReady; }
  async connect(settings: ConnectSettings) {
    const next = { ...settings, realm: normalizeRealm(settings.realm), url: socketUrl(settings.url), org_id: settings.org_id?.trim() || undefined };
    if (next.realm !== 'production' && !next.test_app_secret?.trim()) throw new Error('A test app secret is required for test environments.');
    if (this.settings && (this.opening || this.ready) && (['url', 'realm', 'org_id', 'test_app_secret'] as const).some(key => (this.settings![key] || '') !== (next[key] || ''))) throw new Error('The current connection uses different settings. Wait for it to finish and disconnect before changing environments.');
    this.settings = next;
    if (this.session) {
      try { checkSessionContext(this.session, this.settings); }
      catch (error) { this.disconnect(); this.onExpired(); throw error; }
    }
    if (this.ready && !this.opening) return;
    clearTimeout(this.retry);
    this.closed = false;
    await this.open();
  }
  private async open(): Promise<void> {
    if (this.opening) return this.opening;
    this.opening = this.openConnection().finally(() => { this.opening = undefined; });
    return this.opening;
  }
  private async openConnection() {
    if (!this.settings) throw new Error('A server is required.');
    const settings = this.settings;
    this.helloReady = false;
    this.onStatus(this.session ? 'reconnecting' : 'connecting');
    const ws = new WebSocket(this.settings.url);
    this.socket = ws;
    ws.onmessage = ({ data }) => {
      if (typeof data !== 'string') return;
      let message: any;
      try { message = JSON.parse(data); } catch { return; }
      if (message.id && this.pending.has(message.id)) {
        const request = this.pending.get(message.id)!;
        clearTimeout(request.timer); this.pending.delete(message.id);
        if (message.ok) request.resolve(message.result);
        else { const e = message.error as RingError; request.reject(Object.assign(new Error(`${e.message}${e.next_action ? ` ${e.next_action}` : ''}`), { code: e.code, retryable: e.retryable, details: e.details })); }
      } else if (message.type) for (const listener of this.listeners) listener(message);
    };
    ws.onclose = () => {
      if (this.socket !== ws) return;
      this.helloReady = false;
      for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(new Error('Connection lost. Check the current call before retrying.')); }
      this.pending.clear();
      this.onStatus(this.closed || !this.session ? 'offline' : 'reconnecting');
      for (const listener of this.listeners) listener({ type: 'connection.lost', data: {} });
      if (!this.closed && this.session) this.retry = setTimeout(() => { this.open().catch(() => {}); }, 2500);
    };
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => { ws.close(); reject(new Error('The server did not respond. Check its address and try again.')); }, 10000);
      ws.onopen = () => { clearTimeout(timeout); resolve(); };
      ws.onerror = () => { clearTimeout(timeout); reject(new Error('Could not reach Ring. Make sure the server is running and its address is correct.')); };
    });
    try {
      const hello = await this.request('protocol.hello', { versions: [1], client: { name: 'ring-web', version: '0.1.4' }, realm: settings.realm, org_id: settings.org_id || '', capabilities: ['audio.pcm16', 'events', 'handoff'], ...(settings.realm !== 'production' ? { test_app_secret: settings.test_app_secret } : {}) });
      checkRealm(settings.realm, hello?.realm);
      this.helloReady = true;
      if (this.session) {
        const result = await this.request('auth.resume', { session_token: this.session.session_token, device_id: this.session.device_id });
        this.session = bindSession({ ...this.session, ...result, realm: result.realm }, settings);
        this.onSession(this.session);
        await this.request('events.subscribe', {});
        for (const listener of this.listeners) listener({ type: 'connection.restored', data: {} });
      }
      this.onStatus('connected');
    } catch (error) {
      this.helloReady = false;
      if (isExpiredSession(error)) { this.session = undefined; this.onExpired(); }
      ws.close(); throw error;
    }
  }
  request<T = any>(method: string, params: Record<string, any> = {}, id: string = crypto.randomUUID()): Promise<T> {
    if (!this.ready && !(method === 'protocol.hello' && this.socket?.readyState === WebSocket.OPEN)) return Promise.reject(new Error('You are offline. Reconnect to Ring and try again.'));
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`${method} timed out. Refresh the current state before retrying.`)); }, method === 'voicemail.begin' ? 120000 : 20000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket!.send(JSON.stringify({ id, method, params }));
    });
  }
  frame(type: string, data: Record<string, any>) {
    if (this.ready && this.socket!.bufferedAmount < 256000) this.socket!.send(JSON.stringify({ type, data }));
  }
  disconnect() {
    this.closed = true; this.helloReady = false; this.session = undefined; clearTimeout(this.retry); this.socket?.close(); this.onStatus('offline');
  }
}

export async function downloadAsset(api: RingSocket, asset_id: string): Promise<Blob> {
  const chunks = new Map<number, Uint8Array>();
  const early: any[] = [];
  let transfer = ''; let type = 'application/octet-stream';
  let resolveDone!: (blob: Blob) => void; let rejectDone!: (error: Error) => void;
  const done = new Promise<Blob>((resolve, reject) => { resolveDone = resolve; rejectDone = reject; });
  let timeout: ReturnType<typeof setTimeout>;
  const receive = (frame: any) => {
    const data = frame.data;
    if (frame.type !== 'assets.chunk' || data.transfer_id !== transfer) return;
    if (data.data_base64) chunks.set(data.seq, Uint8Array.from(atob(data.data_base64), ch => ch.charCodeAt(0)));
    if (data.final) {
      const ordered = [...chunks.entries()].sort((a, b) => a[0] - b[0]);
      if (ordered.some(([seq], i) => seq !== i)) rejectDone(new Error('Download has missing chunks. Please try again.'));
      else resolveDone(new Blob(ordered.map(([, bytes]) => bytes as BlobPart), { type }));
    }
  };
  const unsubscribe = api.subscribe(frame => { if (!transfer) early.push(frame); else receive(frame); });
  try {
    const result = await api.request('assets.get', { asset_id });
    if (result.data_base64) return new Blob([Uint8Array.from(atob(result.data_base64), ch => ch.charCodeAt(0))], { type: result.mime_type });
    transfer = result.transfer_id; type = result.mime_type || type;
    timeout = setTimeout(() => rejectDone(new Error('Audio download timed out. Please try again.')), 30000);
    for (const frame of early) receive(frame);
    return await done;
  } finally { unsubscribe(); clearTimeout(timeout!); }
}

export async function uploadAsset(api: RingSocket, file: File, purpose: string): Promise<string> {
  const result = await api.request('assets.begin', { purpose, mime_type: file.type, size_bytes: file.size });
  const bytes = new Uint8Array(await file.arrayBuffer());
  for (let offset = 0, seq = 0; offset < bytes.length; offset += 48000, seq++) {
    let binary = ''; for (const byte of bytes.slice(offset, offset + 48000)) binary += String.fromCharCode(byte);
    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => { unsubscribe(); reject(new Error('The photo upload timed out. Try again.')); }, 15000);
      const unsubscribe = api.subscribe(event => {
        if (event.data?.asset_id !== result.asset_id) return;
        if (event.type === 'assets.ack' && event.data.next_seq === seq + 1) { clearTimeout(timeout); unsubscribe(); resolve(); }
        else if (event.type === 'stream.error') { clearTimeout(timeout); unsubscribe(); reject(new Error(event.data.error?.message || 'Upload failed.')); }
      });
      api.frame('assets.chunk', { asset_id: result.asset_id, seq, data_base64: btoa(binary) });
    });
  }
  await api.request('assets.complete', { asset_id: result.asset_id });
  return result.asset_id;
}

export async function readTranscriptSince(api: Pick<RingSocket, 'request'>, ringid: string, afterSeq = 0): Promise<{ items: any[]; latest_seq: number }> {
  const items: any[] = [], cursors = new Set<string>();
  let cursor: string | undefined, latest = afterSeq;
  do {
    const page = await api.request('transcript.list', { ringid, after_seq: afterSeq, limit: 200, ...(cursor ? { cursor } : {}) });
    items.push(...(page.items || []));
    latest = Math.max(latest, page.latest_seq || 0, ...(page.items || []).map((entry: any) => entry.seq || 0));
    cursor = page.next_cursor || undefined;
    if (cursor && cursors.has(cursor)) throw new Error('The server repeated a transcript cursor. Reconnect and try again.');
    if (cursor) cursors.add(cursor);
  } while (cursor);
  return { items, latest_seq: latest };
}

export function coalesceTranscript(entries: any[]): any[] {
  const latest = new Map<string, number>();
  const result: any[] = [];
  for (const entry of entries) {
    const segment = entry.segment_id || entry.data?.segment_id;
    const key = segment ? `${entry.actor || ''}:${segment}` : '';
    if (key && latest.has(key)) {
      const position = latest.get(key)!;
      const previous = result[position];
      if ((entry.revision ?? entry.data?.revision ?? entry.seq) >= (previous.revision ?? previous.data?.revision ?? previous.seq)) result[position] = entry;
    } else { if (key) latest.set(key, result.length); result.push(entry); }
  }
  return result;
}
export function defaultSocketUrl(hostname: string, native: boolean, development: boolean) {
  return (!native || development) && ['localhost', '127.0.0.1', '[::1]'].includes(hostname)
    ? 'ws://127.0.0.1:8765/ws'
    : 'wss://backend.ring.teamofsilicons.com/ws';
}
