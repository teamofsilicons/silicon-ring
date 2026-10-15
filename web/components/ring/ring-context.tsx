'use client';

import { useRouter } from 'next/navigation';
import { createContext, useContext, useEffect, useState, useSyncExternalStore, type ReactNode } from 'react';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { RingStore, type RingState } from '@/lib/store';

const StoreContext = createContext<RingStore | null>(null);

export function RingProvider({ children }: { children: ReactNode }) {
  const [store] = useState(() => new RingStore());
  const { toast } = useToastStack();
  const router = useRouter();

  useEffect(() => {
    store.onNotice(notice => toast({ type: notice.type, title: notice.title, description: notice.description }));
    store.start();
  }, [store, toast]);

  // The service worker asks an open Ring tab to show a call when its notification is clicked.
  useEffect(() => {
    if (!('serviceWorker' in navigator)) return;
    const onMessage = (event: MessageEvent) => {
      const data = event.data as { type?: string; url?: string } | null;
      // Only paths on this origin: "//host" would leave it.
      if (data?.type === 'ring.open' && typeof data.url === 'string' && /^\/(?![/\\])/.test(data.url)) router.push(data.url);
    };
    navigator.serviceWorker.addEventListener('message', onMessage);
    return () => navigator.serviceWorker.removeEventListener('message', onMessage);
  }, [router]);

  return <StoreContext.Provider value={store}>{children}</StoreContext.Provider>;
}

export function useRingStore() {
  const store = useContext(StoreContext);
  if (!store) throw new Error('useRingStore must be used inside RingProvider.');
  return store;
}

export function useRing<T>(selector: (state: RingState) => T): T {
  const store = useRingStore();
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()), () => selector(store.getState()));
}
