// Ring's local sounds (assets/sounds). Browsers block audio until the page has had a user gesture, so the first
// pointer or key press unlocks every element once; a loop that was asked for before that starts as soon as it can.

export type LoopSound = 'ringback' | 'ringtone';
export type CueSound = 'connected' | 'ended';
type SoundName = LoopSound | CueSound;

const SOURCES: Record<SoundName, string> = {
  ringback: '/sounds/ringback.wav',
  ringtone: '/sounds/ringtone.wav',
  connected: '/sounds/connected.wav',
  ended: '/sounds/ended.wav',
};

class Sounds {
  private elements = new Map<SoundName, HTMLAudioElement>();
  private wanted = new Set<LoopSound>();
  private unlocked = false;
  private listening = false;

  /** Installs the one-time gesture listener. Safe to call more than once. */
  install() {
    if (this.listening || typeof window === 'undefined') return;
    this.listening = true;
    const unlock = () => {
      window.removeEventListener('pointerdown', unlock, true);
      window.removeEventListener('keydown', unlock, true);
      this.unlock();
    };
    window.addEventListener('pointerdown', unlock, true);
    window.addEventListener('keydown', unlock, true);
  }

  get isUnlocked() {
    return this.unlocked;
  }

  private element(name: SoundName) {
    let element = this.elements.get(name);
    if (!element) {
      element = new Audio(SOURCES[name]);
      element.preload = 'auto';
      element.loop = name === 'ringback' || name === 'ringtone';
      this.elements.set(name, element);
    }
    return element;
  }

  private unlock() {
    if (this.unlocked) return;
    this.unlocked = true;
    for (const name of Object.keys(SOURCES) as SoundName[]) {
      const element = this.element(name);
      if (this.wanted.has(name as LoopSound)) continue;
      element.muted = true;
      element.play().then(() => {
        element.muted = false;
        // A loop asked for while this priming play was in flight keeps playing.
        if (this.wanted.has(name as LoopSound)) return;
        element.pause();
        element.currentTime = 0;
      }, () => {
        element.muted = false;
      });
    }
    for (const name of this.wanted) this.start(name);
  }

  private start(name: LoopSound) {
    const element = this.element(name);
    element.muted = false;
    if (!element.paused) return;
    element.currentTime = 0;
    element.play().catch(() => {
      // Still blocked: the gesture listener will retry on the first interaction.
      this.unlocked = false;
      this.listening = false;
      this.install();
    });
  }

  /** Makes exactly these loops play (an empty list stops them all). */
  setLoops(loops: LoopSound[]) {
    const next = new Set(loops);
    for (const name of this.wanted) {
      if (next.has(name)) continue;
      const element = this.elements.get(name);
      if (element) {
        element.pause();
        element.currentTime = 0;
      }
    }
    this.wanted = next;
    for (const name of next) this.start(name);
  }

  play(name: CueSound) {
    const element = this.element(name);
    element.currentTime = 0;
    element.play().catch(() => undefined);
  }
}

export const sounds = new Sounds();
