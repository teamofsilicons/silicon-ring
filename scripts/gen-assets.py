#!/usr/bin/env python3
"""Generate Ring's shared sounds and icon PNGs (assets/). Requires rsvg-convert for PNGs."""
import math, os, struct, subprocess, sys, wave

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
RATE = 44100


def write_wav(name, samples, rate=RATE):
    path = os.path.join(ROOT, "sounds", name)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(struct.pack("<h", max(-32767, min(32767, int(s * 32767)))) for s in samples))
    print("wrote", os.path.relpath(path))


def env(i, n, fade):
    """Linear fade in/out over `fade` samples."""
    return min(1.0, i / fade, (n - i) / fade) if fade else 1.0


def tone(freqs, seconds, amp=0.35, fade_ms=10, rate=RATE):
    n = int(seconds * rate)
    fade = int(fade_ms * rate / 1000)
    return [amp * env(i, n, fade) * sum(math.sin(2 * math.pi * f * i / rate) for f in freqs) / len(freqs) for i in range(n)]


def silence(seconds, rate=RATE):
    return [0.0] * int(seconds * rate)


def marimba(freq, seconds, amp=0.45, rate=RATE):
    n = int(seconds * rate)
    out = []
    for i in range(n):
        t = i / rate
        attack = min(1.0, t / 0.004)
        s = math.sin(2 * math.pi * freq * t) * math.exp(-t * 6.0)
        s += 0.35 * math.sin(2 * math.pi * freq * 4.0 * t) * math.exp(-t * 18.0)
        s += 0.12 * math.sin(2 * math.pi * freq * 9.9 * t) * math.exp(-t * 40.0)
        out.append(amp * attack * s)
    return out


def mix_at(base, clip, start_s, rate=RATE):
    off = int(start_s * rate)
    for i, s in enumerate(clip):
        if off + i < len(base):
            base[off + i] += s
    return base


def main():
    os.makedirs(os.path.join(ROOT, "sounds"), exist_ok=True)
    # Ringback: "tring tring, pause" — two 0.4 s dual tones 0.2 s apart, 3 s cycle.
    ring = tone([440, 480], 0.4, amp=0.3)
    write_wav("ringback.wav", ring + silence(0.2) + ring + silence(3.0 - 1.0))
    # Ringtone: soft marimba figure, 3 s cycle.
    rt = silence(3.0)
    for start, f in [(0.0, 783.99), (0.16, 659.25), (0.32, 783.99), (0.48, 987.77), (1.10, 783.99), (1.26, 659.25)]:
        mix_at(rt, marimba(f, 0.9), start)
    peak = max(abs(s) for s in rt) or 1
    write_wav("ringtone.wav", [s / peak * 0.8 for s in rt])
    # Voicemail beep.
    write_wav("beep.wav", tone([1000], 0.4, amp=0.4))
    # Connected / ended ticks.
    write_wav("connected.wav", tone([880], 0.06, amp=0.25, fade_ms=5) + tone([1318.5], 0.08, amp=0.25, fade_ms=5))
    write_wav("ended.wav", tone([659.25], 0.08, amp=0.25, fade_ms=5) + tone([440], 0.12, amp=0.25, fade_ms=5))

    icon_dir = os.path.join(ROOT, "icon")
    for svg, sizes in [("icon.svg", [16, 20, 24, 29, 32, 40, 48, 58, 60, 64, 76, 80, 87, 96, 120, 128, 152, 167, 180, 192, 256, 512, 1024]), ("icon-macos.svg", [16, 32, 64, 128, 256, 512, 1024]), ("foreground.svg", [108, 162, 216, 324, 432]), ("monochrome.svg", [16, 24, 32, 48, 64, 96, 128, 256])]:
        src = os.path.join(icon_dir, svg)
        stem = svg[:-4]
        for s in sizes:
            out = os.path.join(icon_dir, "png", f"{stem}-{s}.png")
            os.makedirs(os.path.dirname(out), exist_ok=True)
            subprocess.run(["rsvg-convert", "-w", str(s), "-h", str(s), src, "-o", out], check=True)
    print("icons rendered")


if __name__ == "__main__":
    sys.exit(main())
