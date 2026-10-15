'use client';

import { useEffect, useRef, useState } from 'react';
import { Pause, Play } from 'lucide-react';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { Slider } from '@teamofsilicons/silicon-ui/slider';
import { duration } from '@/lib/format';
import { Pixel } from './identity';
import styles from './details.module.css';

/**
 * A player with a scrubber for recordings and voicemail. `load` fetches a fresh presigned URL on first play, because
 * those URLs expire after a few minutes.
 */
export function AudioPlayer({ label, load, knownDurationMs }: { label: string; load: () => Promise<string | null>; knownDurationMs?: number | null }) {
  const audio = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);
  const [position, setPosition] = useState(0);
  const [total, setTotal] = useState((knownDurationMs ?? 0) / 1000);
  const [failed, setFailed] = useState(false);
  const [ready, setReady] = useState(false);

  useEffect(() => () => {
    audio.current?.pause();
    audio.current = null;
  }, []);

  async function element() {
    if (audio.current) return audio.current;
    const src = await load();
    if (!src) return null;
    const next = new Audio(src);
    next.preload = 'auto';
    next.addEventListener('timeupdate', () => setPosition(next.currentTime));
    next.addEventListener('loadedmetadata', () => { if (Number.isFinite(next.duration)) setTotal(next.duration); });
    next.addEventListener('play', () => setPlaying(true));
    next.addEventListener('pause', () => setPlaying(false));
    next.addEventListener('ended', () => { setPlaying(false); setPosition(0); next.currentTime = 0; });
    next.addEventListener('error', () => { setFailed(true); setPlaying(false); setReady(false); audio.current = null; });
    audio.current = next;
    setReady(true);
    return next;
  }

  async function toggle() {
    setLoading(true);
    setFailed(false);
    try {
      const player = await element();
      if (!player) {
        setFailed(true);
        return;
      }
      if (player.paused) await player.play();
      else player.pause();
    } catch {
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className={styles.player}>
      <Button variant="secondary" aria-label={`${playing ? 'Pause' : 'Play'} ${label.toLowerCase()}`} loading={loading} onClick={() => void toggle()}>
        {playing ? <Pause size={16} strokeWidth={1.75} aria-hidden="true" /> : <Play size={16} strokeWidth={1.75} aria-hidden="true" />}
      </Button>
      <div className={styles.scrubber}>
        <Slider
          label={label}
          min={0}
          max={Math.max(total, 1)}
          step={0.1}
          value={Math.min(position, Math.max(total, 1))}
          format={value => duration(value * 1000)}
          disabled={!ready}
          onValueChange={value => {
            setPosition(value);
            if (audio.current) audio.current.currentTime = value;
          }}
        />
      </div>
      <Pixel className={styles.playerTime}>{failed ? 'Unavailable' : `${duration(position * 1000)} / ${duration(total * 1000)}`}</Pixel>
    </div>
  );
}

/** A compact play button for list rows. */
export function InlinePlay({ label, load }: { label: string; load: () => Promise<string | null> }) {
  const audio = useRef<HTMLAudioElement | null>(null);
  const [playing, setPlaying] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => () => {
    audio.current?.pause();
    audio.current = null;
  }, []);

  async function toggle() {
    if (audio.current) {
      if (audio.current.paused) await audio.current.play().catch(() => undefined);
      else audio.current.pause();
      return;
    }
    setLoading(true);
    try {
      const src = await load();
      if (!src) return;
      const next = new Audio(src);
      next.addEventListener('play', () => setPlaying(true));
      next.addEventListener('pause', () => setPlaying(false));
      next.addEventListener('ended', () => setPlaying(false));
      audio.current = next;
      await next.play();
    } catch {
      setPlaying(false);
    } finally {
      setLoading(false);
    }
  }

  return (
    <Button variant="ghost" size="sm" aria-label={playing ? `Pause ${label}` : `Play ${label}`} loading={loading} onClick={() => void toggle()}>
      {playing ? <Pause size={16} strokeWidth={1.75} aria-hidden="true" /> : <Play size={16} strokeWidth={1.75} aria-hidden="true" />}
    </Button>
  );
}
