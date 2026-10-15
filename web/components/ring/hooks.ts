'use client';

import { useEffect, useMemo, useState } from 'react';
import { incomingRings } from '@/lib/store';
import { useRing } from './ring-context';

/** The server's current time (this browser's clock corrected by the realtime socket), refreshed every `intervalMs`. */
export function useNow(intervalMs = 1000) {
  const offset = useRing(state => state.clockOffset);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now + offset;
}

export function useIncomingRings() {
  const calls = useRing(state => state.calls);
  const me = useRing(state => state.me);
  const dismissed = useRing(state => state.dismissed);
  const now = useRing(state => state.now);
  return useMemo(() => incomingRings({ calls, me, dismissed, now }), [calls, me, dismissed, now]);
}

export function useMe() {
  return useRing(state => state.me);
}

/** The call this browser is carrying audio for, if any. */
export function useMediaCall() {
  const media = useRing(state => state.media);
  const call = useRing(state => (state.media ? state.calls[state.media.ringid] : undefined));
  return { media, call };
}

/** Tracks a CSS media query on the client. */
export function useMediaQuery(query: string) {
  const [matches, setMatches] = useState(false);
  useEffect(() => {
    const list = matchMedia(query);
    const update = () => setMatches(list.matches);
    update();
    list.addEventListener('change', update);
    return () => list.removeEventListener('change', update);
  }, [query]);
  return matches;
}
