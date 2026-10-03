import { RingSocket } from './protocol';
const encode = (buffer: ArrayBuffer) => {
  const bytes = new Uint8Array(buffer); let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};
export class CallAudio {
  streamId = ''; muted = false; lastSeq = 0;
  private context?: AudioContext;
  private microphone?: MediaStream;
  private worklet?: AudioWorkletNode;
  private unsubscribe?: () => void;
  private outputAt = 0;
  private seq = 0;
  private speechSeq = 0;
  private offset = 0;
  onLevel: (level: number) => void = () => {};
  async start(api: RingSocket, ringid: string, device_id: string, voicemail_id?: string) {
    if (this.streamId) await this.stop(api);
    if (!navigator.mediaDevices?.getUserMedia) throw new Error('Microphone access requires HTTPS or localhost.');
    this.microphone = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true }, video: false });
    try {
      this.context = new AudioContext({ sampleRate: 24000 });
      await this.context.resume();
      await this.context.audioWorklet.addModule('/pcm-worklet.js');
      const attachment = await api.request('media.attach', { ringid, device_id, purpose: voicemail_id ? 'voicemail' : 'call', ...(voicemail_id ? { voicemail_id } : {}) });
      this.streamId = attachment.stream_id; this.seq = 0; this.speechSeq = 0; this.offset = 0; this.lastSeq = 0; this.outputAt = 0; this.muted = false;
      this.worklet = new AudioWorkletNode(this.context, 'ring-pcm');
      this.worklet.port.onmessage = ({ data }) => {
        const offset = this.offset; this.offset += 20;
        if (!this.streamId || !api.ready || this.muted) return;
        const seq = this.seq++; this.lastSeq = seq;
        api.frame('media.audio', { stream_id: this.streamId, seq, offset_ms: offset, audio_base64: encode(data.buffer) });
        // ponytail: energy VAD is a lightweight fallback; a learned VAD can improve noisy-room attribution.
        if (data.rms > 0.018) api.frame('media.speech', { stream_id: this.streamId, seq: ++this.speechSeq, start_ms: offset, end_ms: offset + 20, confidence: Math.min(1, data.rms * 10) });
        this.onLevel(Math.min(1, data.rms * 6));
      };
      this.context.createMediaStreamSource(this.microphone).connect(this.worklet);
      // A silent sink keeps capture processing alive without microphone feedback.
      const sink = this.context.createGain(); sink.gain.value = 0; this.worklet.connect(sink); sink.connect(this.context.destination);
      this.unsubscribe = api.subscribe(event => {
        if (event.type === 'media.audio' && event.data.stream_id === this.streamId && event.data.audio_base64) this.play(event.data.audio_base64);
        if (event.type === 'connection.lost') void this.stop();
      });
    } catch (error) { await this.stop(); throw error; }
  }
  private play(encoded: string) {
    if (!this.context || this.context.state === 'closed') return;
    const binary = atob(encoded); const samples = new Int16Array(binary.length / 2);
    const bytes = new DataView(samples.buffer);
    for (let i = 0; i + 1 < binary.length; i += 2) bytes.setInt16(i, binary.charCodeAt(i) | binary.charCodeAt(i + 1) << 8, true);
    const buffer = this.context.createBuffer(1, samples.length, 24000);
    const channel = buffer.getChannelData(0);
    for (let i = 0; i < samples.length; i++) channel[i] = samples[i] / 32768;
    const node = this.context.createBufferSource(); node.buffer = buffer; node.connect(this.context.destination);
    const now = this.context.currentTime;
    if (this.outputAt < now || this.outputAt > now + 0.3) this.outputAt = now + 0.04;
    node.start(this.outputAt); this.outputAt += buffer.duration;
  }
  async mute(api: RingSocket, muted: boolean) {
    if (!this.streamId) throw new Error('Connect your microphone first.');
    await api.request('media.state', { stream_id: this.streamId, muted });
    this.muted = muted;
    this.microphone?.getAudioTracks().forEach(track => { track.enabled = !muted; });
    if (muted) this.onLevel(0);
  }
  async stop(api?: RingSocket, voicemail = false) {
    const stream_id = this.streamId; this.streamId = ''; this.unsubscribe?.(); this.unsubscribe = undefined;
    this.microphone?.getTracks().forEach(track => track.stop()); this.microphone = undefined;
    if (this.worklet) this.worklet.port.onmessage = null; this.worklet?.disconnect(); this.worklet = undefined;
    await this.context?.close(); this.context = undefined; this.onLevel(0);
    if (api?.ready && stream_id) return api.request('media.detach', { stream_id, ...(voicemail ? { last_seq: this.lastSeq } : {}) });
  }
}
export class RingTone {
  private context?: AudioContext; private interval?: ReturnType<typeof setInterval>;
  async start(incoming: boolean) {
    this.stop(); this.context = new AudioContext(); await this.context.resume();
    const ring = () => {
      if (!this.context || this.context.state !== 'running') return;
      for (const offset of [0, 0.28]) {
        const oscillator = this.context.createOscillator(); const gain = this.context.createGain();
        oscillator.frequency.value = incoming ? 660 : 440;
        gain.gain.setValueAtTime(0, this.context.currentTime + offset);
        gain.gain.linearRampToValueAtTime(0.055, this.context.currentTime + offset + 0.02);
        gain.gain.linearRampToValueAtTime(0, this.context.currentTime + offset + 0.19);
        oscillator.connect(gain); gain.connect(this.context.destination);
        oscillator.start(this.context.currentTime + offset); oscillator.stop(this.context.currentTime + offset + 0.2);
      }
    };
    ring(); this.interval = setInterval(ring, 3000);
  }
  stop() { clearInterval(this.interval); void this.context?.close(); this.context = undefined; }
}
