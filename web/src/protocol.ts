export type RingError = { code: string; message: string; next_action?: string; retryable?: boolean };
export type RingEvent = { type: string; data: any; seq?: number; event_id?: string };
export type Session = { actor: string; actor_id?: string; display_name?: string; org_id: string; device_id: string; session_token: string; expires_at?: string };
export type Call = { ringid: string; caller: string; target: string; state: string; created_at: string; answered_at?: string; ended_at?: string; outcome?: string; recording_status?: string; participants: { actor: string; display_name?: string; device_id?: string; left_at?: string | null }[]; invitations: { invitation_id: string; inviter: string; target: string; state: string; reason?: string; expires_at: string }[] };
export const normalizeActor = (value: string) => value.trim().replace(/^@/, '');
export const isActor = (value: string) => /^(?:c|si):[^\s\[\]]+(?:\[[^\s\[\]]+\])?$/.test(normalizeActor(value));
export const displayActor = (value = '') => value.replace(/^(?:c|si):/, '').replace(/\[.*\]$/, '');
export function socketUrl(value: string) {
  const url = new URL(value);
  if (!['ws:', 'wss:'].includes(url.protocol) || url.username || url.password) throw new Error('Enter a WebSocket URL starting with ws:// or wss://.');
  if (url.protocol === 'ws:' && !['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) throw new Error('Use wss:// for remote servers. Plain WebSockets are allowed only on this device.');
  return url.toString();
}
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

export class RingSocket {
  private socket?: WebSocket;
  private pending = new Map<string, { resolve: (result: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  private listeners = new Set<(event: RingEvent) => void>();
  private retry?: ReturnType<typeof setTimeout>;
  private closed = false;
  private opening?: Promise<void>;
  private settings?: { url: string; realm: string; org_id?: string; test_app_secret?: string };
  session?: Session;
  onStatus: (state: 'offline' | 'connecting' | 'connected' | 'reconnecting') => void = () => {};
  onExpired: () => void = () => {};
  subscribe(listener: (event: RingEvent) => void) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  get ready() { return this.socket?.readyState === WebSocket.OPEN; }
  async connect(settings: { url: string; realm: string; org_id?: string; test_app_secret?: string }) {
    this.settings = { ...settings, url: socketUrl(settings.url) };
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
        else { const e = message.error as RingError; request.reject(Object.assign(new Error(`${e.message}${e.next_action ? ` ${e.next_action}` : ''}`), { code: e.code })); }
      } else if (message.type) for (const listener of this.listeners) listener(message);
    };
    ws.onclose = () => {
      if (this.socket !== ws) return;
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
      await this.request('protocol.hello', { versions: [1], client: { name: 'ring-web', version: '0.1.0' }, realm: this.settings.realm, org_id: this.settings.org_id || undefined, capabilities: ['audio.pcm16', 'events', 'handoff'], ...(this.settings.test_app_secret ? { test_app_secret: this.settings.test_app_secret } : {}) });
      if (this.session) {
        const result = await this.request('auth.resume', { session_token: this.session.session_token, device_id: this.session.device_id });
        this.session = { ...this.session, ...result };
        await this.request('events.subscribe', {});
        for (const listener of this.listeners) listener({ type: 'connection.restored', data: {} });
      }
      this.onStatus('connected');
    } catch (error) {
      if ((error as any).code?.includes('AUTH') || (error as any).code?.includes('SESSION')) { this.session = undefined; this.onExpired(); }
      ws.close(); throw error;
    }
  }
  request<T = any>(method: string, params: Record<string, any> = {}, id: string = crypto.randomUUID()): Promise<T> {
    if (!this.ready) return Promise.reject(new Error('You are offline. Reconnect to Ring and try again.'));
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
    this.closed = true; this.session = undefined; clearTimeout(this.retry); this.socket?.close(); this.onStatus('offline');
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
