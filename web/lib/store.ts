import { ApiError, api, errorMessage, fullTranscript, isUnauthenticated } from './api';
import { browserDeviceName, deviceLabel, label, myParticipant } from './format';
import { CallMedia, type MediaStatus } from './media';
import {
  closeNotifications, pushPermission, pushToken, requestNotifications, showPageNotification, unsubscribePush, type PushState,
} from './push';
import { RealtimeConnection, type RealtimeStatus } from './realtime';
import { sounds, type LoopSound } from './sounds';
import type { Account, Call, CallState, DeviceRef, DeviceUpdate, Invitation, MediaGrant, Me, Meta, RealtimeEvent, TranscriptEntry } from './types';

export const APP_VERSION = process.env.NEXT_PUBLIC_RING_VERSION ?? '1.0.0';

/** The label ring-server gives a session created by web sign-in, before the browser names itself. */
const SERVER_DEFAULT_DEVICE_NAME = 'Web browser';

export interface PartialLine {
  segmentId: string;
  speaker: Account;
  text: string;
}

export interface LiveTranscript {
  entries: TranscriptEntry[];
  partials: PartialLine[];
  synced: boolean;
}

export interface MediaState {
  ringid: string;
  status: MediaStatus;
  muted: boolean;
  speaking: string[];
  audioBlocked: boolean;
  micError: string | null;
}

/**
 * A call the server still has on this browser while this tab carries no audio for it, as after a reload: it is being
 * rejoined on its own (`joining`), or waits for a click on Rejoin call (`offered`).
 */
export interface Resume {
  ringid: string;
  state: 'joining' | 'offered';
}

export interface Notice {
  type: 'success' | 'info' | 'warning' | 'error';
  title: string;
  description?: string;
}

export interface RingState {
  status: 'loading' | 'signed-out' | 'signed-in' | 'unreachable';
  me: Me | null;
  meta: Meta | null;
  connection: RealtimeStatus | 'idle';
  calls: Record<string, Call>;
  transcripts: Record<string, LiveTranscript>;
  media: MediaState | null;
  /** Calls this tab carried until they moved to another of the account's devices, with that device's name. */
  movedTo: Record<string, string>;
  resume: Resume | null;
  /** Invitations this tab stopped showing (answered, declined or silenced here, or stopped elsewhere). */
  dismissed: Record<string, true>;
  unreadVoicemail: number;
  revision: { calls: number; voicemails: number; devices: number };
  push: PushState;
  pushActive: boolean;
  /** The server's clock minus this browser's, in ms, so ring expiry and call timers don't depend on a correct local clock. */
  clockOffset: number;
  /** Server time, refreshed when a ring expires. */
  now: number;
}

export interface IncomingRing {
  call: Call;
  invitation: Invitation;
}

const LIVE_STATES = new Set(['ringing', 'active', 'voicemail']);

// A call only moves forward (ringing, then active or voicemail, then ended). A REST read that started before a realtime
// update can land after it; it must not move the call back, or a finished ringback would start again.
const CALL_STAGE: Record<CallState, number> = { ringing: 0, active: 1, voicemail: 1, ended: 2 };

/** Invitations ringing for this account that this tab should show now. */
export function incomingRings(state: Pick<RingState, 'calls' | 'me' | 'dismissed' | 'now'>): IncomingRing[] {
  const me = state.me;
  if (!me || me.account.kind !== 'carbon' || !me.device.ring_enabled) return [];
  const rings: IncomingRing[] = [];
  for (const call of Object.values(state.calls)) {
    if (call.state !== 'ringing' && call.state !== 'active') continue;
    for (const invitation of call.invitations) {
      if (invitation.to.uuid !== me.account.uuid || invitation.state !== 'ringing' || invitation.silenced) continue;
      if (state.dismissed[invitation.invitation_id] || Date.parse(invitation.expires_at) <= state.now) continue;
      rings.push({ call, invitation });
    }
  }
  return rings.sort((a, b) => Date.parse(a.invitation.created_at) - Date.parse(b.invitation.created_at));
}

function initialState(): RingState {
  return {
    status: 'loading',
    me: null,
    meta: null,
    connection: 'idle',
    calls: {},
    transcripts: {},
    media: null,
    movedTo: {},
    resume: null,
    dismissed: {},
    unreadVoicemail: 0,
    revision: { calls: 0, voicemails: 0, devices: 0 },
    push: 'unsupported',
    pushActive: false,
    clockOffset: 0,
    now: Date.now(),
  };
}

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

// The tab carrying a call's audio holds this Web Lock until it lets go, so other tabs of the same browser (the same
// Ring device) can tell the call is already here.
const MEDIA_LOCK = 'ring.media';

function holdMediaLock(): () => void {
  if (typeof navigator === 'undefined' || !navigator.locks) return () => undefined;
  let release: () => void = () => undefined;
  const held = new Promise<void>(resolve => {
    release = resolve;
  });
  navigator.locks.request(MEDIA_LOCK, () => held).catch(() => undefined);
  return release;
}

async function mediaHeldByAnotherTab() {
  if (typeof navigator === 'undefined' || !navigator.locks) return false;
  try {
    const { held } = await navigator.locks.query();
    return Boolean(held?.some(lock => lock.name === MEDIA_LOCK));
  } catch {
    return false;
  }
}

/** Whether this site may use the microphone without asking, so a call can be rejoined without a click. */
async function microphoneGranted() {
  try {
    const status = await navigator.permissions.query({ name: 'microphone' as PermissionName });
    return status.state === 'granted';
  } catch {
    // Browsers that can't say (or don't know the name) ask on use, which needs the person's click anyway.
    return false;
  }
}

function speechSegment(entry: TranscriptEntry) {
  const id = entry.kind === 'speech' ? entry.data?.segment_id : undefined;
  return typeof id === 'string' && id ? id : null;
}

/**
 * Everything live in the web app: the session, the realtime socket, calls in progress, their transcripts, and the one
 * call whose audio this tab carries. React reads it through useSyncExternalStore; the socket and REST both write to it.
 */
export class RingStore {
  private state = initialState();
  private listeners = new Set<() => void>();
  private realtime: RealtimeConnection | null = null;
  private media: CallMedia | null = null;
  private releaseMediaLock: (() => void) | null = null;
  private expiryTimer: ReturnType<typeof setTimeout> | null = null;
  private notified = new Set<string>();
  /** Invitations this tab is answering, so their own "stopped ringing" doesn't read as answered elsewhere. */
  private accepting = new Set<string>();
  private rejoining = new Set<string>();
  private lastSessionCheck = 0;
  private notice: (notice: Notice) => void = () => undefined;
  private started = false;

  getState = () => this.state;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  onNotice(handler: (notice: Notice) => void) {
    this.notice = handler;
  }

  private set(update: Partial<RingState> | ((state: RingState) => Partial<RingState>)) {
    const previous = this.state;
    const patch = typeof update === 'function' ? update(previous) : update;
    this.state = { ...previous, ...patch };
    this.afterChange(previous);
    for (const listener of this.listeners) listener();
  }

  // ------------------------------------------------------------------------------------------------ lifecycle

  start() {
    if (this.started) return;
    this.started = true;
    sounds.install();
    void this.load();
  }

  /** Reads the session. `quiet` keeps the current screen up while it does (background retries). */
  async load(quiet = false) {
    if (!quiet) this.set({ status: 'loading' });
    const metaRead = api.meta().catch(() => null);
    try {
      const me = await api.me();
      const meta = await metaRead;
      this.set({ status: 'signed-in', me, meta, push: pushPermission() });
      if (me.account.kind === 'carbon') {
        this.connectRealtime();
        void this.syncDevice();
        void this.refreshUnread();
      }
    } catch (error) {
      const meta = await metaRead;
      if (isUnauthenticated(error)) this.set({ status: 'signed-out', me: null, meta });
      else this.set({ status: 'unreachable', meta });
    }
  }

  private connectRealtime() {
    this.realtime?.stop();
    this.realtime = new RealtimeConnection(
      event => this.handleEvent(event),
      connection => {
        this.set({ connection });
        if (connection === 'reconnecting') void this.checkSession();
      },
      () => void this.reconcile(),
    );
    this.realtime.start();
  }

  /** A socket that keeps failing may mean the session ended; REST says which. */
  private async checkSession() {
    if (Date.now() - this.lastSessionCheck < 30_000) return;
    this.lastSessionCheck = Date.now();
    try {
      await api.me();
    } catch (error) {
      if (isUnauthenticated(error)) this.signedOut();
    }
  }

  private signedOut() {
    this.realtime?.stop();
    this.realtime = null;
    this.media?.disconnect();
    this.releaseMediaLock?.();
    this.releaseMediaLock = null;
    sounds.setLoops([]);
    this.set({ ...initialState(), status: 'signed-out', meta: this.state.meta });
  }

  // ------------------------------------------------------------------------------------------------ derived effects

  private afterChange(previous: RingState) {
    const state = this.state;
    if (typeof window === 'undefined') return;
    const rings = incomingRings(state);

    const loops: LoopSound[] = [];
    const mediaCall = state.media ? state.calls[state.media.ringid] : undefined;
    if (mediaCall && mediaCall.state === 'ringing' && mediaCall.created_by.uuid === state.me?.account.uuid) loops.push('ringback');
    if (rings.length && !state.media) loops.push('ringtone');
    sounds.setLoops(loops);

    if (previous.media && mediaCall && previous.calls[mediaCall.ringid]?.state === 'ringing' && mediaCall.state === 'active') sounds.play('connected');

    const ringing = new Set(rings.map(ring => ring.invitation.invitation_id));
    for (const ring of incomingRings(previous)) {
      if (!ringing.has(ring.invitation.invitation_id)) void closeNotifications(ring.call.ringid);
    }
    this.scheduleExpiry(rings);
  }

  private serverNow() {
    return Date.now() + this.state.clockOffset;
  }

  private scheduleExpiry(rings: IncomingRing[]) {
    if (this.expiryTimer) clearTimeout(this.expiryTimer);
    this.expiryTimer = null;
    if (!rings.length) return;
    const next = Math.min(...rings.map(ring => Date.parse(ring.invitation.expires_at)));
    this.expiryTimer = setTimeout(() => this.set({ now: this.serverNow() }), Math.max(250, next - this.serverNow() + 50));
  }

  /** `ready` and `pong` are sent the moment they are due, so their `at` is a fair reading of the server's clock. */
  private observeServerClock(at: string) {
    const server = Date.parse(at);
    if (!Number.isFinite(server)) return;
    const offset = server - Date.now();
    if (Math.abs(offset - this.state.clockOffset) < 1000) return;
    this.set({ clockOffset: offset, now: Date.now() + offset });
  }

  // ------------------------------------------------------------------------------------------------ realtime

  private handleEvent(event: RealtimeEvent) {
    if ((event.type === 'ready' || event.type === 'pong') && event.at) this.observeServerClock(event.at);
    switch (event.type) {
      case 'call.incoming':
        this.upsertCall(event.data.call);
        this.notifyIncoming(event.data.call, event.data.invitation);
        break;
      case 'call.updated':
      case 'call.ended':
        this.upsertCall(event.data.call);
        break;
      case 'call.ringing_stopped': {
        const { ringid, invitation_id, reason } = event.data;
        // call.updated (with the invitation no longer ringing) arrives first, so "was this ringing here" is whether
        // this tab was told about it and didn't answer it itself.
        const rangHere = this.notified.has(invitation_id) && !this.state.dismissed[invitation_id] && !this.accepting.has(invitation_id);
        this.set(state => ({ dismissed: { ...state.dismissed, [invitation_id]: true } }));
        void closeNotifications(ringid);
        if (rangHere && reason === 'accepted_elsewhere' && this.state.media?.ringid !== ringid) {
          this.notice({ type: 'info', title: 'Answered on another device' });
        }
        if (rangHere && reason === 'missed') {
          const caller = event.data.from ?? this.state.calls[ringid]?.invitations.find(invitation => invitation.invitation_id === invitation_id)?.from;
          this.notifyMissed(ringid, caller ?? null);
        }
        break;
      }
      case 'call.handoff':
        this.handleHandoff(event.data.ringid, event.data.device_id, event.data.device ?? null);
        break;
      case 'transcript.entry':
        this.addEntries(event.data.ringid, [event.data.entry]);
        break;
      case 'transcript.partial':
        this.addPartial(event.data.ringid, { segmentId: event.data.segment_id, speaker: event.data.speaker, text: event.data.text });
        break;
      case 'voicemail.received': {
        const { voicemail } = event.data;
        this.set(state => ({ unreadVoicemail: state.unreadVoicemail + (voicemail.read ? 0 : 1), revision: { ...state.revision, voicemails: state.revision.voicemails + 1 } }));
        this.notice({ type: 'info', title: `New voicemail from ${label(voicemail.from)}`, description: voicemail.transcript ?? undefined });
        if (document.visibilityState === 'hidden' && !this.state.pushActive) {
          void showPageNotification(`Voicemail from ${label(voicemail.from)}`, { body: voicemail.transcript ?? 'New voicemail', tag: voicemail.voicemail_id, icon: '/icons/icon-192.png', data: { kind: 'voicemail', url: `/voicemail?id=${encodeURIComponent(voicemail.voicemail_id)}` } });
        }
        break;
      }
      case 'voicemail.updated':
        this.set(state => ({ revision: { ...state.revision, voicemails: state.revision.voicemails + 1 } }));
        void this.refreshUnread();
        break;
      case 'device.updated': {
        const me = this.state.me;
        if (me && event.data.device.device_id === me.device.device_id) {
          const device = event.data.device;
          this.set({
            me: { ...me, device: { ...device, current: true } },
            pushActive: pushPermission() === 'granted' && device.push.some(entry => entry.kind === 'webpush'),
          });
        }
        this.set(state => ({ revision: { ...state.revision, devices: state.revision.devices + 1 } }));
        break;
      }
      case 'session.revoked':
        this.notice({ type: 'warning', title: 'This browser was signed out' });
        this.signedOut();
        break;
      default:
        break;
    }
  }

  private async reconcile() {
    try {
      const [me, live, meta] = await Promise.all([
        api.me(),
        api.calls({ state: 'ringing,active,voicemail', direction: 'all', limit: 50 }),
        this.state.meta ? Promise.resolve(this.state.meta) : api.meta().catch(() => null),
      ]);
      const firstMeta = !this.state.meta && meta;
      this.set({ me, meta });
      // Without /v1/meta at startup there was no VAPID key to subscribe with.
      if (firstMeta) void this.syncDevice();
      const seen = new Set(live.items.map(call => call.ringid));
      for (const call of live.items) this.upsertCall(call);
      const stale = Object.values(this.state.calls).filter(call => LIVE_STATES.has(call.state) && !seen.has(call.ringid));
      await Promise.all(stale.map(call => api.call(call.ringid).then(fresh => this.upsertCall(fresh), () => undefined)));
      for (const call of live.items) {
        if (call.state === 'active' && myParticipant(call, me.account.uuid)?.state === 'joined') void this.syncTranscript(call.ringid);
      }
      void this.autoRejoin();
      void this.refreshUnread();
      this.set(state => ({ revision: { ...state.revision, calls: state.revision.calls + 1 } }));
    } catch (error) {
      if (isUnauthenticated(error)) this.signedOut();
    }
  }

  /** Whether the server has this browser as the device carrying my side of a live call. */
  private carriedHere(call: Call | undefined) {
    const me = this.state.me;
    if (!me || !call || !LIVE_STATES.has(call.state)) return false;
    const mine = myParticipant(call, me.account.uuid);
    return mine?.state === 'joined' && mine.device?.device_id === me.device.device_id;
  }

  /**
   * After a reload, picks the call back up when this browser is still its device (the server keeps it for 30 s). With
   * the microphone already allowed that happens on its own; otherwise the screens offer Rejoin call, because the
   * permission prompt and the call's sound both need the person's click. Every tab of a browser is the same device, so
   * a tab never takes the call from another tab that is carrying it.
   */
  private async autoRejoin() {
    if (this.state.media) return;
    const call = Object.values(this.state.calls).find(candidate => this.carriedHere(candidate));
    if (!call || this.state.resume?.ringid === call.ringid) return;
    const ringid = call.ringid;
    if (await mediaHeldByAnotherTab()) return;
    const granted = await microphoneGranted();
    // Another reconcile, or a click on Rejoin call, may have picked it up meanwhile.
    if (this.state.media || this.state.resume?.ringid === ringid || this.rejoining.has(ringid)) return;
    if (!this.carriedHere(this.state.calls[ringid])) return;
    if (!granted) {
      this.set({ resume: { ringid, state: 'offered' } });
      return;
    }
    this.set({ resume: { ringid, state: 'joining' } });
    await this.rejoin(ringid);
    this.settleResume(ringid);
  }

  /** After a rejoin attempt: done once this tab has the call, otherwise offered again while the call is still on this browser. */
  private settleResume(ringid: string) {
    if (this.state.resume?.ringid !== ringid) return;
    const offered = !this.state.media && this.carriedHere(this.state.calls[ringid]);
    this.set({ resume: offered ? { ringid, state: 'offered' } : null });
  }

  private upsertCall(call: Call) {
    const previous = this.state.calls[call.ringid];
    if (previous && CALL_STAGE[call.state] < CALL_STAGE[previous.state]) return;
    this.set(state => ({ calls: { ...state.calls, [call.ringid]: call } }));
    if (this.state.resume?.ringid === call.ringid && !this.carriedHere(call)) this.set({ resume: null });
    const media = this.state.media;
    if (media?.ringid === call.ringid) {
      const mine = myParticipant(call, this.state.me?.account.uuid);
      const over = call.state === 'ended' || mine?.state === 'left';
      if (over) this.endMedia(true);
    }
    if (previous?.state !== call.state || call.state === 'ended') {
      this.set(state => ({ revision: { ...state.revision, calls: state.revision.calls + 1 } }));
    }
  }

  private handleHandoff(ringid: string, deviceId: string, device: DeviceRef | null) {
    const me = this.state.me;
    if (!me || deviceId === me.device.device_id) return;
    const targetIn = (call: Call | undefined) => {
      const ref = call ? myParticipant(call, me.account.uuid)?.device : null;
      return ref && ref.device_id === deviceId ? ref : null;
    };
    // Newer servers name the device in the event; older ones only in the call update, which can come after it.
    const target = (device?.device_id === deviceId ? device : null) ?? targetIn(this.state.calls[ringid]);
    const moved = this.state.media?.ringid === ringid;
    if (moved) {
      this.endMedia(false);
      this.set(state => ({ movedTo: { ...state.movedTo, [ringid]: deviceLabel(target) } }));
      if (target) this.notice({ type: 'info', title: `Moved to ${deviceLabel(target)}` });
    }
    void api.call(ringid).then(fresh => {
      this.upsertCall(fresh);
      return targetIn(fresh);
    }, () => null).then(named => {
      // The handoff event can arrive before the call update that names the new device.
      if (!moved || target) return;
      if (named && this.state.movedTo[ringid]) this.set(state => ({ movedTo: { ...state.movedTo, [ringid]: deviceLabel(named) } }));
      this.notice({ type: 'info', title: `Moved to ${deviceLabel(named)}` });
    });
  }

  private notifyIncoming(call: Call, invitation: Invitation) {
    if (this.notified.has(invitation.invitation_id)) return;
    this.notified.add(invitation.invitation_id);
    if (!incomingRings(this.state).some(ring => ring.invitation.invitation_id === invitation.invitation_id)) return;
    if (document.visibilityState !== 'hidden' || this.state.pushActive) return;
    const others = call.participants.filter(p => p.state === 'joined' && p.account.uuid !== invitation.from.uuid).map(p => label(p.account));
    const lead = invitation.from.kind === 'silicon' ? 'Silicon calling you' : 'Calling you';
    void showPageNotification(label(invitation.from), {
      body: others.length ? `${lead} with ${others.join(', ')} · ${invitation.from.id}` : `${lead} · ${invitation.from.id}`,
      tag: call.ringid,
      icon: invitation.from.pfp_url ?? '/icons/icon-192.png',
      badge: '/icons/badge-96.png',
      requireInteraction: true,
      // Same shape as the push notifications in public/sw.js, so its Answer and Decline handling applies.
      data: { kind: 'call', ringid: call.ringid, invitation_id: invitation.invitation_id, expires_at: invitation.expires_at },
      actions: [{ action: 'answer', title: 'Answer' }, { action: 'decline', title: 'Decline' }],
    });
  }

  /** A call that rang here and wasn't answered: a toast on the open page, a notification when the tab is hidden. */
  private notifyMissed(ringid: string, caller: Account | null) {
    const title = `Missed call from ${label(caller)}`;
    if (document.visibilityState !== 'hidden') {
      this.notice({ type: 'info', title });
      return;
    }
    // With Web Push the service worker shows the server's missed-call push instead.
    if (this.state.pushActive) return;
    void showPageNotification(title, {
      body: caller?.id ?? '',
      // Its own tag, so closing the ringing notification (tagged with the ringid) never closes this one.
      tag: `${ringid}:missed`,
      icon: caller?.pfp_url ?? '/icons/icon-192.png',
      badge: '/icons/badge-96.png',
      data: { kind: 'missed', url: `/history?id=${encodeURIComponent(ringid)}` },
    });
  }

  // ------------------------------------------------------------------------------------------------ transcripts

  /**
   * Final speech replaces its interim line: the one with the entry's segment_id, along with that speaker's older
   * interim lines (a speaker's segments finish in order, so those lost their final). A newer segment that is
   * already being spoken stays. Without a segment_id (older servers) the speaker's interim lines all go.
   */
  private addEntries(ringid: string, entries: TranscriptEntry[]) {
    this.set(state => {
      const current = state.transcripts[ringid] ?? { entries: [], partials: [], synced: false };
      const bySeq = new Map(current.entries.map(entry => [entry.seq, entry]));
      for (const entry of entries) bySeq.set(entry.seq, entry);
      let partials = current.partials;
      for (const entry of entries) {
        const speaker = entry.kind === 'speech' ? entry.actor?.uuid : undefined;
        if (!speaker) continue;
        const segment = speechSegment(entry);
        const index = segment ? partials.findIndex(line => line.segmentId === segment) : -1;
        if (segment && index < 0) continue;
        partials = partials.filter((line, i) => i !== index && (line.speaker.uuid !== speaker || (index >= 0 && i > index)));
      }
      return {
        transcripts: {
          ...state.transcripts,
          [ringid]: { ...current, entries: [...bySeq.values()].sort((a, b) => a.seq - b.seq), partials },
        },
      };
    });
  }

  /** Interim speech, one line per segment: an update keeps the line's place, a new segment goes last. */
  private addPartial(ringid: string, partial: PartialLine) {
    this.set(state => {
      const current = state.transcripts[ringid] ?? { entries: [], partials: [], synced: false };
      // A partial that arrives after its own final would bring the line back as interim.
      if (current.entries.slice(-50).some(entry => speechSegment(entry) === partial.segmentId)) return {};
      const index = current.partials.findIndex(line => line.segmentId === partial.segmentId);
      let partials: PartialLine[];
      if (!partial.text.trim()) partials = current.partials.filter(line => line.segmentId !== partial.segmentId);
      else if (index >= 0) partials = current.partials.map((line, i) => (i === index ? partial : line));
      else partials = [...current.partials, partial];
      return { transcripts: { ...state.transcripts, [ringid]: { ...current, partials } } };
    });
  }

  async syncTranscript(ringid: string) {
    const current = this.state.transcripts[ringid];
    const after = current?.entries.at(-1)?.seq ?? 0;
    try {
      const entries = await fullTranscript(ringid, current?.synced ? after : 0);
      this.addEntries(ringid, entries);
      this.set(state => ({ transcripts: { ...state.transcripts, [ringid]: { ...state.transcripts[ringid]!, synced: true } } }));
    } catch {
      // The live stream keeps adding entries; a later reconnect retries the backfill.
    }
  }

  // ------------------------------------------------------------------------------------------------ media

  private mediaEngine() {
    this.media ??= new CallMedia({
      onStatus: status => this.updateMedia({ status }),
      onSpeakers: speaking => this.updateMedia({ speaking }),
      onAudioBlocked: audioBlocked => this.updateMedia({ audioBlocked }),
      onMicrophoneError: micError => this.updateMedia({ micError }),
      onLost: () => {
        const ringid = this.state.media?.ringid;
        if (ringid) void this.rejoin(ringid);
      },
      onRemoved: () => {
        const ringid = this.state.media?.ringid;
        if (!ringid) return;
        this.endMedia(false);
        void api.call(ringid).then(fresh => this.upsertCall(fresh), () => undefined);
      },
    });
    return this.media;
  }

  private updateMedia(patch: Partial<MediaState>) {
    if (!this.state.media) return;
    this.set(state => (state.media ? { media: { ...state.media, ...patch } } : {}));
  }

  private async connectMedia(ringid: string, grant: MediaGrant) {
    const muted = this.state.media?.ringid === ringid ? this.state.media.muted : false;
    this.releaseMediaLock ??= holdMediaLock();
    this.set(state => {
      const movedTo = { ...state.movedTo };
      delete movedTo[ringid];
      return { movedTo, resume: null, media: { ringid, status: 'connecting', muted, speaking: [], audioBlocked: false, micError: null } };
    });
    try {
      await this.mediaEngine().connect(grant, muted);
    } catch (error) {
      if (this.state.media?.ringid !== ringid) return;
      this.updateMedia({ status: 'disconnected' });
      this.notice({ type: 'error', title: 'Audio could not connect', description: errorMessage(error) });
    }
  }

  private endMedia(cue: boolean) {
    if (!this.state.media) return;
    this.media?.disconnect();
    this.releaseMediaLock?.();
    this.releaseMediaLock = null;
    this.set({ media: null });
    if (cue) sounds.play('ended');
  }

  /** Gets a fresh media grant for this browser and joins with it, retrying through short network drops. */
  private async rejoin(ringid: string) {
    if (this.rejoining.has(ringid)) return;
    this.rejoining.add(ringid);
    try {
      for (let attempt = 0; attempt < 4; attempt++) {
        const call = this.state.calls[ringid];
        if (call && !LIVE_STATES.has(call.state)) return;
        try {
          const result = await api.media(ringid);
          this.upsertCall(result.call);
          if (result.media) await this.connectMedia(ringid, result.media);
          return;
        } catch (error) {
          if (error instanceof ApiError && !error.retryable) {
            if (this.state.media?.ringid === ringid) this.endMedia(false);
            return;
          }
          await sleep(1500 * (attempt + 1));
        }
      }
    } finally {
      this.rejoining.delete(ringid);
    }
  }

  // ------------------------------------------------------------------------------------------------ actions

  async startCall(to: string, idempotencyKey: string) {
    if (this.state.media) throw new Error('You’re already on a call. Hang up first.');
    const created = await api.createCall(to, idempotencyKey);
    this.upsertCall(created.call);
    if (created.media) void this.connectMedia(created.call.ringid, created.media);
    return created.call;
  }

  async accept(ringid: string, invitationId?: string) {
    const current = this.state.media;
    if (current && current.ringid !== ringid) await this.leave(current.ringid);
    if (invitationId) this.accepting.add(invitationId);
    let result;
    try {
      result = await api.accept(ringid, invitationId);
    } finally {
      if (invitationId) this.accepting.delete(invitationId);
    }
    if (invitationId) this.set(state => ({ dismissed: { ...state.dismissed, [invitationId]: true } }));
    this.upsertCall(result.call);
    // The call screen opens right away and shows "Connecting…" while LiveKit joins.
    if (result.media) void this.connectMedia(ringid, result.media);
    void this.syncTranscript(ringid);
    return result.call;
  }

  async decline(ringid: string, invitationId?: string, reason?: string) {
    if (invitationId) this.set(state => ({ dismissed: { ...state.dismissed, [invitationId]: true } }));
    try {
      const result = await api.decline(ringid, invitationId, reason);
      this.upsertCall(result.call);
    } catch (error) {
      if (!(error instanceof ApiError && (error.code === 'invitation_not_ringing' || error.code === 'invitation_expired'))) throw error;
    }
  }

  async silence(ringid: string, invitationId?: string) {
    if (invitationId) this.set(state => ({ dismissed: { ...state.dismissed, [invitationId]: true } }));
    try {
      const result = await api.silence(ringid, invitationId);
      this.upsertCall(result.call);
    } catch (error) {
      if (!(error instanceof ApiError && (error.code === 'invitation_not_ringing' || error.code === 'invitation_expired'))) throw error;
    }
  }

  async leave(ringid: string) {
    if (this.state.media?.ringid === ringid) this.endMedia(true);
    try {
      const result = await api.leave(ringid);
      this.upsertCall(result.call);
    } catch (error) {
      if (error instanceof ApiError && (error.code === 'call_ended' || error.code === 'not_participant')) return;
      throw error;
    }
  }

  async invite(ringid: string, to: string, idempotencyKey: string) {
    const result = await api.invite(ringid, to, idempotencyKey);
    this.upsertCall(result.call);
    return result.invitation;
  }

  async switchHere(ringid: string) {
    const current = this.state.media;
    if (current && current.ringid !== ringid) throw new Error('Hang up your other call first.');
    // Marked as rejoining so a reconnect's automatic rejoin doesn't ask for a second grant meanwhile.
    this.rejoining.add(ringid);
    try {
      const result = await api.media(ringid);
      this.upsertCall(result.call);
      if (result.media) await this.connectMedia(ringid, result.media);
    } finally {
      this.rejoining.delete(ringid);
    }
    void this.syncTranscript(ringid);
  }

  /** Rejoin call: takes back a call this browser carried before the page reloaded. Run it from a click. */
  async resumeCall(ringid: string) {
    this.set({ resume: { ringid, state: 'joining' } });
    try {
      await this.switchHere(ringid);
    } finally {
      this.settleResume(ringid);
    }
  }

  async setMuted(muted: boolean) {
    this.updateMedia({ muted });
    try {
      await this.media?.setMuted(muted);
    } catch {
      this.updateMedia({ muted: true });
    }
  }

  async startAudio() {
    await this.media?.startAudio();
  }

  ensureCall(ringid: string) {
    if (this.state.calls[ringid]) return Promise.resolve(this.state.calls[ringid]);
    return api.call(ringid).then(call => {
      this.upsertCall(call);
      return call;
    });
  }

  async refreshUnread() {
    try {
      const page = await api.voicemails({ unread: true, limit: 100 });
      this.set({ unreadVoicemail: page.items.filter(item => !item.read).length });
    } catch {
      // Keep the last count.
    }
  }

  async refreshMe() {
    const me = await api.me();
    this.set({ me });
    return me;
  }

  async updateThisDevice(patch: { name?: string; ring_enabled?: boolean }) {
    const me = this.state.me;
    if (!me) return;
    const device = await api.patchDevice(me.device.device_id, patch);
    this.set(state => ({ me: state.me ? { ...state.me, device: { ...device, current: true } } : null, revision: { ...state.revision, devices: state.revision.devices + 1 } }));
  }

  /** Registers this browser as a web device, with its push subscription when notifications are allowed. */
  async syncDevice() {
    const { me, meta } = this.state;
    if (!me) return;
    const permission = pushPermission();
    // Without permission this browser can't show a pushed call, so its token goes. With permission, a subscription that
    // couldn't be read this time (no VAPID key yet, the push service unreachable) leaves the registered one in place.
    let push: DeviceUpdate['push'];
    if (permission === 'granted') {
      const token = await pushToken(meta?.webpush_public_key ?? null).catch(() => null);
      if (token) push = [{ kind: 'webpush', token }];
    } else {
      push = [];
    }
    // The server names every new web session generically; a name like "Chrome on macOS" tells devices apart.
    const unnamed = !me.device.name?.trim() || me.device.name === SERVER_DEFAULT_DEVICE_NAME;
    try {
      const device = await api.updateCurrentDevice({
        platform: 'web',
        app_version: APP_VERSION,
        ...(unnamed ? { name: browserDeviceName() } : {}),
        ...(push ? { push } : {}),
      });
      const pushActive = permission === 'granted' && device.push.some(entry => entry.kind === 'webpush');
      this.set(state => ({ me: state.me ? { ...state.me, device: { ...device, current: true } } : null, pushActive, push: permission }));
    } catch {
      this.set({ push: permission });
    }
  }

  async enableNotifications() {
    const result = await requestNotifications();
    this.set({ push: result });
    if (result === 'granted') await this.syncDevice();
    return result;
  }

  bumpVoicemails() {
    this.set(state => ({ revision: { ...state.revision, voicemails: state.revision.voicemails + 1 } }));
    void this.refreshUnread();
  }

  async signOut() {
    try {
      await unsubscribePush();
    } catch {
      // The server forgets this device's tokens with the session anyway.
    }
    try {
      await api.signOut();
    } catch {
      // Signing out locally still matters if the server is unreachable.
    }
    this.signedOut();
  }
}
