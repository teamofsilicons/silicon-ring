import {
  type AudioCaptureOptions, createLocalAudioTrack, DisconnectReason, type LocalAudioTrack, type Participant,
  type RemoteTrack, Room, RoomEvent, Track,
} from 'livekit-client';
import { Preroll } from './preroll';
import type { MediaGrant } from './types';

export type MediaStatus = 'connecting' | 'connected' | 'reconnecting' | 'disconnected';

export interface MediaHandlers {
  onStatus: (status: MediaStatus) => void;
  onSpeakers: (identities: string[]) => void;
  onAudioBlocked: (blocked: boolean) => void;
  onMicrophoneError: (message: string | null) => void;
  /** The room closed without this tab asking it to, in a way worth rejoining (network loss). */
  onLost: () => void;
  /** The server removed this participant, or another tab joined with the same identity: the audio is no longer here. */
  onRemoved: () => void;
}

function microphoneMessage(error: unknown) {
  const name = error instanceof DOMException ? error.name : '';
  if (name === 'NotAllowedError' || name === 'SecurityError') return 'Ring can’t use your microphone. Allow it in your browser’s site settings.';
  if (name === 'NotFoundError' || name === 'OverconstrainedError') return 'No microphone was found.';
  if (name === 'NotReadableError') return 'Your microphone is in use by another app.';
  return 'Your microphone could not be started.';
}

// Mono speech at 48 kHz through the browser's echo cancellation, noise suppression and gain control. Voice isolation
// (LiveKit turns it on by default) replaces noise suppression with a much stronger filter where the browser has one;
// it stays off so quiet syllables reach the transcript and the representative.
const CAPTURE: AudioCaptureOptions = {
  echoCancellation: true, noiseSuppression: true, autoGainControl: true, voiceIsolation: false, channelCount: 1, sampleRate: 48_000,
};

/**
 * The LiveKit side of one call on this browser: join with the server's media grant, publish exactly one microphone
 * track, play every remote audio track, and report who is speaking by LiveKit identity (matched to participants'
 * media_identity by the caller).
 */
export class CallMedia {
  private room: Room | null = null;
  private leaving = false;
  private muted = false;
  private preroll: Preroll | null = null;
  private readonly sink: HTMLDivElement;

  constructor(private readonly handlers: MediaHandlers) {
    this.sink = document.createElement('div');
    this.sink.hidden = true;
    this.sink.dataset.ringAudio = '';
    document.body.appendChild(this.sink);
  }

  get identity() {
    return this.room?.localParticipant.identity ?? null;
  }

  async connect(grant: MediaGrant, muted: boolean) {
    this.disconnect();
    this.leaving = false;
    this.muted = muted;
    const room = new Room({
      adaptiveStream: false,
      dynacast: false,
      disconnectOnPageLeave: true,
      audioCaptureDefaults: CAPTURE,
      // Opus at 32 kbps (above LiveKit's 24 kbps speech preset) with RED, so a lost packet is rebuilt from the next one.
      // DTX stays off: it stops sending in pauses and clips the first syllable when speech resumes.
      publishDefaults: { audioPreset: { maxBitrate: 32_000 }, dtx: false, red: true },
    });
    this.room = room;
    room
      .on(RoomEvent.TrackSubscribed, (track: RemoteTrack) => {
        if (track.kind !== Track.Kind.Audio) return;
        const element = track.attach();
        this.sink.appendChild(element);
      })
      .on(RoomEvent.TrackUnsubscribed, (track: RemoteTrack) => {
        for (const element of track.detach()) element.remove();
      })
      .on(RoomEvent.ActiveSpeakersChanged, (speakers: Participant[]) => {
        this.handlers.onSpeakers(speakers.map(speaker => speaker.identity));
      })
      .on(RoomEvent.AudioPlaybackStatusChanged, () => {
        this.handlers.onAudioBlocked(!room.canPlaybackAudio);
      })
      .on(RoomEvent.Reconnecting, () => this.handlers.onStatus('reconnecting'))
      .on(RoomEvent.Reconnected, () => this.handlers.onStatus('connected'))
      .on(RoomEvent.Disconnected, (reason?: DisconnectReason) => {
        if (this.room !== room) return;
        this.room = null;
        this.handlers.onStatus('disconnected');
        this.handlers.onSpeakers([]);
        if (this.leaving || reason === DisconnectReason.CLIENT_INITIATED || reason === DisconnectReason.ROOM_DELETED) return;
        if (reason === DisconnectReason.PARTICIPANT_REMOVED || reason === DisconnectReason.DUPLICATE_IDENTITY) this.handlers.onRemoved();
        else this.handlers.onLost();
      });

    this.handlers.onStatus('connecting');
    // The microphone starts while the room is joined rather than after: what it hears meanwhile (often the first words
    // after answering) is the pre-roll, which ring-media puts in front of the published track. Joining does not wait
    // for it, nor for a permission prompt.
    const starting = !muted && grant.preroll_to ? this.startMicrophone() : Promise.resolve(null);
    try {
      await room.connect(grant.url, grant.token, { autoSubscribe: true });
    } catch (error) {
      void starting.then(early => this.stopMicrophone(early));
      throw error;
    }
    if (this.room !== room) {
      void starting.then(early => this.stopMicrophone(early));
      return;
    }
    this.handlers.onStatus('connected');
    this.handlers.onAudioBlocked(!room.canPlaybackAudio);
    const early = await starting;
    if (this.room !== room) {
      this.stopMicrophone(early);
      return;
    }
    this.preroll = early?.preroll ?? null;
    try {
      if (early && grant.preroll_to) {
        await early.preroll?.open(room, grant.preroll_to);
        if (this.muted) {
          early.preroll?.stop();
          await early.track.mute();
        }
        await room.localParticipant.publishTrack(early.track, { source: Track.Source.Microphone });
        early.preroll?.published(room);
      } else {
        await room.localParticipant.setMicrophoneEnabled(!this.muted);
      }
      this.handlers.onMicrophoneError(null);
    } catch (error) {
      this.stopMicrophone(early);
      this.handlers.onMicrophoneError(microphoneMessage(error));
    }
  }

  /** The microphone track, already listening for the pre-roll; null when it cannot start yet (the published
   * track then starts the usual way, and reports why it cannot). */
  private async startMicrophone() {
    const [track, preroll] = await Promise.all([createLocalAudioTrack(CAPTURE).catch(() => null), Preroll.prepare()]);
    if (!track) {
      preroll?.stop();
      return null;
    }
    preroll?.listen(track.mediaStreamTrack);
    return { track, preroll };
  }

  private stopMicrophone(early: { track: LocalAudioTrack; preroll: Preroll | null } | null) {
    early?.preroll?.stop();
    void early?.track.stop();
  }

  async setMuted(muted: boolean) {
    this.muted = muted;
    // Nothing more is heard once muted, pre-roll included.
    if (muted) this.preroll?.stop();
    const room = this.room;
    if (!room) return;
    try {
      await room.localParticipant.setMicrophoneEnabled(!muted);
      this.handlers.onMicrophoneError(null);
    } catch (error) {
      this.handlers.onMicrophoneError(microphoneMessage(error));
      throw error;
    }
  }

  /** Must run inside a user gesture when the browser blocked autoplay. */
  async startAudio() {
    await this.room?.startAudio();
    if (this.room) this.handlers.onAudioBlocked(!this.room.canPlaybackAudio);
  }

  disconnect() {
    this.preroll?.stop();
    this.preroll = null;
    const room = this.room;
    if (!room) return;
    this.leaving = true;
    this.room = null;
    void room.disconnect(true);
    for (const element of Array.from(this.sink.children)) element.remove();
    this.handlers.onStatus('disconnected');
    this.handlers.onSpeakers([]);
  }

  dispose() {
    this.disconnect();
    this.sink.remove();
  }
}
