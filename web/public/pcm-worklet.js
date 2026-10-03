// Resample hardware audio to the negotiated 24 kHz PCM stream, in 20 ms frames.
class RingPCM extends AudioWorkletProcessor {
  constructor() { super(); this.samples = []; this.position = 0; this.buffer = new Int16Array(480); this.count = 0; this.energy = 0; }
  process(inputs) {
    const input = inputs[0]?.[0];
    if (!input) return true;
    for (const sample of input) this.samples.push(sample);
    const ratio = sampleRate / 24000;
    while (this.position + 1 < this.samples.length) {
      const index = Math.floor(this.position), fraction = this.position - index;
      const sample = Math.max(-1, Math.min(1, this.samples[index] * (1 - fraction) + this.samples[index + 1] * fraction));
      this.buffer[this.count++] = Math.round(sample * (sample < 0 ? 32768 : 32767));
      this.energy += sample * sample;
      this.position += ratio;
      if (this.count === 480) {
        this.port.postMessage({ buffer: this.buffer.buffer, rms: Math.sqrt(this.energy / 480) }, [this.buffer.buffer]);
        this.buffer = new Int16Array(480); this.count = 0; this.energy = 0;
      }
    }
    const consumed = Math.floor(this.position);
    this.samples.splice(0, consumed); this.position -= consumed;
    return true;
  }
}
registerProcessor('ring-pcm', RingPCM);
