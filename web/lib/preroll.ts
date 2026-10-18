import { ParticipantEvent, type ByteStreamWriter, type Room } from 'livekit-client';

/** LiveKit's topic for pre-connect audio, which ring-media reads. */
const TOPIC = 'lk.agent.pre-connect-audio-buffer';
/** Full speech bandwidth at half the bytes of 48 kHz, which matters on a mobile uplink. */
const RATE = 24_000;
/** After this device's track is first subscribed the pre-roll runs on this long, so it overlaps the live audio
 * ring-media lines it up with. */
const TAIL_MS = 600;
/** It ends this long after publishing in any case: LiveKit does not tell a publisher that the (hidden) recorder
 * subscribed. */
const LIMIT_MS = 4_000;
/** The most kept while connecting. */
const MAX_SAMPLES = RATE * 15;
/** Audio is sent in pieces of about this many samples. */
const SEND_SAMPLES = RATE / 10;

const WORKLET = `registerProcessor('ring-preroll', class extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel) this.port.postMessage(channel.slice(0));
    return true;
  }
});`;

/**
 * The microphone's pre-roll: what it hears from the moment the call is answered, while the call's audio connects
 * and before the published track reaches anyone. It streams to ring-media (`MediaGrant.preroll_to`) as LiveKit
 * pre-connect audio (16-bit little-endian PCM), and ring-media puts it in front of the live track, so the first
 * words reach the recording, the transcript and a silicon's representative.
 */
export class Preroll {
  private pending: Int16Array[] = [];
  private pendingSamples = 0;
  private writer: ByteStreamWriter | null = null;
  private sending: Promise<void> = Promise.resolve();
  private limit: ReturnType<typeof setTimeout> | undefined;
  private stopped = false;

  private source: MediaStreamAudioSourceNode | null = null;

  private constructor(
    private readonly context: AudioContext,
    private readonly node: AudioWorkletNode,
  ) {
    node.port.onmessage = (event: MessageEvent<Float32Array>) => this.capture(event.data);
  }

  /** Gets ready to listen, while the microphone starts; null where the browser cannot. */
  static async prepare(): Promise<Preroll | null> {
    let context: AudioContext | null = null;
    try {
      try {
        context = new AudioContext({ sampleRate: RATE });
      } catch {
        context = new AudioContext();
      }
      const url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
      try {
        await context.audioWorklet.addModule(url);
      } finally {
        URL.revokeObjectURL(url);
      }
      // No outputs: it only listens, and plays nothing.
      const node = new AudioWorkletNode(context, 'ring-preroll', { numberOfInputs: 1, numberOfOutputs: 0, channelCount: 1 });
      if (context.state === 'suspended') await context.resume().catch(() => undefined);
      return new Preroll(context, node);
    } catch {
      void context?.close();
      return null;
    }
  }

  /** Listens to the microphone track from now on. */
  listen(track: MediaStreamTrack) {
    if (this.stopped) return;
    this.source = this.context.createMediaStreamSource(new MediaStream([track]));
    this.source.connect(this.node);
  }

  /** Opens the stream to `identity`. Done before the track is published, so ring-media expects the pre-roll by the
   * time the track arrives. */
  async open(room: Room, identity: string) {
    if (this.stopped) return;
    try {
      this.writer = await room.localParticipant.streamBytes({
        topic: TOPIC,
        destinationIdentities: [identity],
        attributes: { sampleRate: String(this.context.sampleRate), channels: '1' },
      });
    } catch {
      this.stop();
      return;
    }
    if (this.stopped) this.close();
    else this.send();
  }

  /** The track went out: the pre-roll ends once it has been heard. */
  published(room: Room) {
    if (this.stopped) return;
    room.localParticipant.once(ParticipantEvent.LocalTrackSubscribed, () => {
      setTimeout(() => this.stop(), TAIL_MS);
    });
    this.limit = setTimeout(() => this.stop(), LIMIT_MS);
  }

  /** Stops listening and closes the stream with whatever was heard. */
  stop() {
    if (this.stopped) return;
    this.stopped = true;
    clearTimeout(this.limit);
    this.node.port.onmessage = null;
    this.source?.disconnect();
    void this.context.close();
    this.close();
  }

  private close() {
    this.send(true);
    const writer = this.writer;
    this.writer = null;
    if (writer) this.sending = this.sending.then(() => writer.close()).catch(() => undefined);
  }

  private capture(data: Float32Array) {
    if (this.stopped) return;
    const samples = new Int16Array(data.length);
    for (let i = 0; i < data.length; i++) samples[i] = Math.max(-1, Math.min(1, data[i]!)) * 0x7fff;
    this.pending.push(samples);
    this.pendingSamples += samples.length;
    while (!this.writer && this.pendingSamples > MAX_SAMPLES) this.pendingSamples -= this.pending.shift()!.length;
    this.send();
  }

  private send(all = false) {
    const writer = this.writer;
    if (!writer || this.pendingSamples === 0 || (!all && this.pendingSamples < SEND_SAMPLES)) return;
    const bytes = new Uint8Array(this.pendingSamples * 2);
    const view = new DataView(bytes.buffer);
    let offset = 0;
    for (const chunk of this.pending) {
      for (const sample of chunk) {
        view.setInt16(offset, sample, true);
        offset += 2;
      }
    }
    this.pending = [];
    this.pendingSamples = 0;
    this.sending = this.sending.then(() => writer.write(bytes)).catch(() => this.stop());
  }
}
