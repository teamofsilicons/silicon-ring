import type { RealtimeEvent } from './types';

export type RealtimeStatus = 'connecting' | 'open' | 'reconnecting' | 'stopped';

// Production serves /v1/ws on the page's own origin. The dev server cannot proxy WebSocket upgrades, so in development
// the socket goes straight to the local ring-server on the same host name (the session cookie is host-only, not per port).
export function realtimeUrl() {
  const configured = process.env.NEXT_PUBLIC_RING_WS_URL;
  if (configured) return configured;
  if (process.env.NODE_ENV === 'development') return `ws://${location.hostname}:${process.env.NEXT_PUBLIC_RING_DEV_WS_PORT ?? '8765'}/v1/ws`;
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/v1/ws`;
}

const PING_EVERY_MS = 25_000;
const SILENCE_LIMIT_MS = 65_000;
const MAX_BACKOFF_MS = 30_000;
const CONNECT_TIMEOUT_MS = 15_000;

/**
 * One realtime connection per tab. It reconnects with jittered backoff, pings within the server's 60 s window, and
 * treats a socket that has been silent past the server's own 20 s pings as dead. `onOpen` runs on every (re)connect so
 * the caller can reconcile with REST.
 */
export class RealtimeConnection {
  private socket: WebSocket | null = null;
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private lastMessageAt = 0;
  private stopped = true;

  constructor(
    private readonly onEvent: (event: RealtimeEvent) => void,
    private readonly onStatus: (status: RealtimeStatus) => void,
    private readonly onOpen: () => void,
  ) {}

  start() {
    if (!this.stopped) return;
    this.stopped = false;
    window.addEventListener('online', this.wake);
    document.addEventListener('visibilitychange', this.wake);
    this.connect();
  }

  stop() {
    this.stopped = true;
    window.removeEventListener('online', this.wake);
    document.removeEventListener('visibilitychange', this.wake);
    this.clearTimers();
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onclose = null;
      socket.close(1000, 'signed out');
    }
    this.onStatus('stopped');
  }

  private wake = () => {
    if (this.stopped || document.visibilityState === 'hidden') return;
    const socket = this.socket;
    if (socket && socket.readyState === WebSocket.OPEN && Date.now() - this.lastMessageAt > SILENCE_LIMIT_MS) this.abandon(socket);
    if (!this.socket || this.socket.readyState === WebSocket.CLOSED) {
      if (this.retryTimer) clearTimeout(this.retryTimer);
      this.retryTimer = null;
      this.connect();
    }
  };

  /**
   * Gives up on a socket that has gone silent. A dead network never completes the close handshake, so waiting for
   * `onclose` could leave the tab deaf for minutes; detach it and reconnect now.
   */
  private abandon(socket: WebSocket) {
    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
    socket.onerror = null;
    try {
      socket.close();
    } catch {
      // Already closing.
    }
    if (this.socket === socket) this.socket = null;
    this.clearPing();
    this.scheduleRetry();
  }

  private connect() {
    if (this.stopped) return;
    this.onStatus(this.attempt === 0 ? 'connecting' : 'reconnecting');
    let socket: WebSocket;
    try {
      socket = new WebSocket(realtimeUrl());
    } catch {
      this.scheduleRetry();
      return;
    }
    this.socket = socket;
    const opening = setTimeout(() => {
      if (this.socket === socket && socket.readyState === WebSocket.CONNECTING) this.abandon(socket);
    }, CONNECT_TIMEOUT_MS);
    socket.onopen = () => {
      clearTimeout(opening);
      this.attempt = 0;
      this.lastMessageAt = Date.now();
      this.onStatus('open');
      this.startPing();
      this.onOpen();
    };
    socket.onmessage = message => {
      this.lastMessageAt = Date.now();
      if (typeof message.data !== 'string') return;
      let frame: RealtimeEvent;
      try {
        frame = JSON.parse(message.data) as RealtimeEvent;
      } catch {
        return;
      }
      if (frame && typeof frame.type === 'string') this.onEvent(frame);
    };
    socket.onclose = () => {
      clearTimeout(opening);
      if (this.socket === socket) this.socket = null;
      this.clearPing();
      if (!this.stopped) this.scheduleRetry();
    };
    socket.onerror = () => socket.close();
  }

  private scheduleRetry() {
    if (this.stopped || this.retryTimer) return;
    this.onStatus('reconnecting');
    const base = Math.min(MAX_BACKOFF_MS, 1000 * 2 ** this.attempt);
    const delay = base / 2 + Math.random() * (base / 2);
    this.attempt = Math.min(this.attempt + 1, 6);
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.connect();
    }, delay);
  }

  private startPing() {
    this.clearPing();
    this.pingTimer = setInterval(() => {
      const socket = this.socket;
      if (!socket || socket.readyState !== WebSocket.OPEN) return;
      if (Date.now() - this.lastMessageAt > SILENCE_LIMIT_MS) {
        this.abandon(socket);
        return;
      }
      socket.send(JSON.stringify({ type: 'ping' }));
    }, PING_EVERY_MS);
  }

  private clearPing() {
    if (this.pingTimer) clearInterval(this.pingTimer);
    this.pingTimer = null;
  }

  private clearTimers() {
    this.clearPing();
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }
}
