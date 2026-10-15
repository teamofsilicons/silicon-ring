'use client';

import { useEffect, useMemo, useRef } from 'react';
import { label } from '@/lib/format';
import type { LiveTranscript } from '@/lib/store';
import type { TranscriptEntry } from '@/lib/types';
import styles from './call.module.css';

const SHOWN = 40;

function systemLine(entry: TranscriptEntry): string | null {
  const who = entry.actor ? label(entry.actor) : 'Someone';
  switch (entry.kind) {
    case 'joined': return `${who} joined`;
    case 'left': return `${who} left`;
    case 'invited': {
      const invitee = entry.data.invitee as { display_name?: string | null; id?: string } | undefined;
      return `${who} invited ${invitee?.display_name || invitee?.id || 'someone'}`;
    }
    case 'declined': return entry.data.reason ? `${who} declined · ${String(entry.data.reason)}` : `${who} declined`;
    case 'note': return entry.text;
    default: return null;
  }
}

/** The live transcript: lines enter at the bottom, move up and fade toward the top. Interim speech stays muted. */
export function LiveTranscriptView({ transcript, meUuid }: { transcript: LiveTranscript | undefined; meUuid: string | undefined }) {
  const scroller = useRef<HTMLDivElement>(null);
  // Follows the newest line until the person scrolls back to read; scrolling to the bottom again resumes it.
  const following = useRef(true);
  const lines = useMemo(() => {
    const entries = (transcript?.entries ?? []).filter(entry => (entry.kind === 'speech' && entry.text?.trim()) || systemLine(entry));
    return entries.slice(-SHOWN);
  }, [transcript?.entries]);
  const partials = useMemo(() => transcript?.partials ?? [], [transcript?.partials]);
  const empty = !lines.length && !partials.length;

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    let last = element.scrollTop;
    const onScroll = () => {
      const atBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
      if (atBottom) following.current = true;
      else if (element.scrollTop < last - 2) following.current = false;
      last = element.scrollTop;
    };
    // The box shrinks when notes such as a microphone warning appear above it; keep the newest line in view.
    const observer = new ResizeObserver(() => {
      if (following.current) element.scrollTop = element.scrollHeight;
    });
    observer.observe(element);
    element.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      observer.disconnect();
      element.removeEventListener('scroll', onScroll);
    };
  }, []);

  useEffect(() => {
    const element = scroller.current;
    if (!element || !following.current) return;
    element.scrollTo({ top: element.scrollHeight, behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' });
  }, [lines.length, partials]);

  return (
    <div ref={scroller} className={styles.transcript} role="log" aria-live="polite" aria-label="Live transcript">
      {empty ? (
        <p className={styles.transcriptEmpty}>The transcript appears here as people speak.</p>
      ) : (
        <div className={styles.transcriptLines}>
          {lines.map(entry => {
            if (entry.kind !== 'speech') {
              return <p key={entry.seq} className={styles.systemLine}>{systemLine(entry)}</p>;
            }
            const mine = entry.actor?.uuid === meUuid;
            return (
              <p key={entry.seq} className={styles.line}>
                <span className={styles.speaker}>{mine ? 'You' : label(entry.actor)}</span>
                <span className={styles.lineText}>{entry.text}</span>
              </p>
            );
          })}
          {partials.map(partial => (
            <p key={partial.segmentId} className={[styles.line, styles.interim].join(' ')}>
              <span className={styles.speaker}>{partial.speaker.uuid === meUuid ? 'You' : label(partial.speaker)}</span>
              <span className={styles.lineText}>{partial.text}</span>
            </p>
          ))}
        </div>
      )}
    </div>
  );
}
