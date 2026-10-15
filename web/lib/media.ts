import {
  DisconnectReason, type Participant, type RemoteTrack, Room, RoomEvent, Track,
} from 'livekit-client';
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

/**
 * The LiveKit side of one call on this browser: join with the server's media grant, publish exactly one microphone
 * track, play every remote audio track, and report who is speaking by LiveKit identity (matched to participants'
 * media_identity by the caller).
 */
export class CallMedia {
  private room: Room | null = null;
  private leaving = false;
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
    const room = new Room({
      adaptiveStream: false,
      dynacast: false,
      disconnectOnPageLeave: true,
      audioCaptureDefaults: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
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
    await room.connect(grant.url, grant.token, { autoSubscribe: true });
    if (this.room !== room) return;
    this.handlers.onStatus('connected');
    this.handlers.onAudioBlocked(!room.canPlaybackAudio);
    try {
      await room.localParticipant.setMicrophoneEnabled(!muted);
      this.handlers.onMicrophoneError(null);
    } catch (error) {
      this.handlers.onMicrophoneError(microphoneMessage(error));
    }
  }

  async setMuted(muted: boolean) {
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
