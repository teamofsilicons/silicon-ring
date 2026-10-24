'use client';

import { useCallback, useEffect, useMemo, useState } from 'react';
import { api, errorMessage } from '@/lib/api';
import { incomingRings } from '@/lib/store';
import type { Account } from '@/lib/types';
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

/**
 * Names the tab for a screen that stands in for the route's own page (sign-in, loading), which would otherwise keep the
 * route's title, such as "Recents". The route's title comes back when the screen goes.
 */
export function useDocumentTitle(title: string) {
  useEffect(() => {
    const previous = document.title;
    document.title = title;
    return () => {
      if (document.title === title) document.title = previous;
    };
  }, [title]);
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

/**
 * The accounts you blocked (`GET /v1/blocks`), with block and unblock that keep the list current. Until the list loads,
 * or when it can't, it is empty and `loaded` is false.
 */
export function useBlocks() {
  const [blocked, setBlocked] = useState<Account[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.blocks().then(
      result => {
        if (cancelled) return;
        setBlocked(result.items);
        setLoaded(true);
        setError(null);
      },
      failure => { if (!cancelled) setError(errorMessage(failure)); },
    );
    return () => { cancelled = true; };
  }, []);

  const block = useCallback(async (account: Account) => {
    await api.block(account.uuid);
    setBlocked(current => (current.some(item => item.uuid === account.uuid) ? current : [account, ...current]));
  }, []);

  const unblock = useCallback(async (account: Account) => {
    await api.unblock(account.uuid);
    setBlocked(current => current.filter(item => item.uuid !== account.uuid));
  }, []);

  const isBlocked = useCallback((account: Pick<Account, 'uuid'>) => blocked.some(item => item.uuid === account.uuid), [blocked]);

  return { blocked, loaded, error, block, unblock, isBlocked };
}
