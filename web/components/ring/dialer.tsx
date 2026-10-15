'use client';

import { useRouter } from 'next/navigation';
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Phone, UserPlus } from 'lucide-react';
import { Badge } from '@teamofsilicons/silicon-ui/badge';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { Dialog, DialogContent } from '@teamofsilicons/silicon-ui/dialog';
import { SearchField } from '@teamofsilicons/silicon-ui/search-field';
import { Skeleton } from '@teamofsilicons/silicon-ui/skeleton';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { ApiError, api, errorMessage, newIdempotencyKey } from '@/lib/api';
import { actorRef, label, when } from '@/lib/format';
import type { Account, AccountLookup, RecentPerson } from '@/lib/types';
import { useIncomingRings } from './hooks';
import { PersonAvatar, Pixel, SiliconBadge } from './identity';
import { useRingStore } from './ring-context';
import styles from './dialer.module.css';

type DialerMode = { kind: 'call' } | { kind: 'invite'; ringid: string; exclude: string[] };

const DialerContext = createContext<(mode?: DialerMode) => void>(() => undefined);

export function useDialer() {
  return useContext(DialerContext);
}

export function DialerProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<DialerMode | null>(null);
  const open = useCallback((next: DialerMode = { kind: 'call' }) => setMode(next), []);
  // An incoming call takes over the screen: a modal dialer left open would trap focus and clicks under the sheet.
  const ringing = useIncomingRings().length > 0;
  const [wasRinging, setWasRinging] = useState(ringing);
  if (ringing !== wasRinging) {
    setWasRinging(ringing);
    if (ringing) setMode(null);
  }
  return (
    <DialerContext.Provider value={open}>
      {children}
      <Dialog open={mode !== null} onOpenChange={next => { if (!next) setMode(null); }}>
        {mode ? (
          <DialogContent
            title={mode.kind === 'call' ? 'New call' : 'Add someone'}
            description={mode.kind === 'call' ? 'Call a carbon or a silicon by their id.' : 'They’ll see who is already on the call before they answer.'}
          >
            <Dialer mode={mode} onDone={() => setMode(null)} />
          </DialogContent>
        ) : null}
      </Dialog>
    </DialerContext.Provider>
  );
}

type Lookup =
  | { state: 'idle' }
  | { state: 'loading'; ref: string }
  | { state: 'found'; ref: string; result: AccountLookup }
  | { state: 'missing'; ref: string }
  | { state: 'error'; ref: string; message: string };

function Dialer({ mode, onDone }: { mode: DialerMode; onDone: () => void }) {
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const [text, setText] = useState('');
  const [lookup, setLookup] = useState<Lookup>({ state: 'idle' });
  const [recent, setRecent] = useState<RecentPerson[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const key = useRef(newIdempotencyKey());
  const ref = actorRef(text);

  useEffect(() => {
    if (!ref) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      setLookup({ state: 'loading', ref });
      api.lookup(ref, controller.signal).then(
        result => setLookup({ state: 'found', ref, result }),
        error => {
          if (controller.signal.aborted) return;
          if (error instanceof ApiError && (error.status === 404 || error.code === 'account_not_found')) setLookup({ state: 'missing', ref });
          else setLookup({ state: 'error', ref, message: errorMessage(error) });
        },
      );
    }, 250);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [ref]);

  useEffect(() => {
    const controller = new AbortController();
    const query = ref ? '' : text.trim();
    const timer = setTimeout(() => {
      api.recentPeople(query, 12, controller.signal).then(
        result => setRecent(result.items),
        () => { if (!controller.signal.aborted) setRecent([]); },
      );
    }, query ? 200 : 0);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [text, ref]);

  const exclude = useMemo(() => new Set(mode.kind === 'invite' ? mode.exclude : []), [mode]);

  async function dial(target: string, account?: Account) {
    if (busy) return;
    setBusy(target);
    try {
      if (mode.kind === 'call') {
        const call = await store.startCall(target, key.current);
        onDone();
        router.push(`/call?id=${encodeURIComponent(call.ringid)}`);
      } else {
        await store.invite(mode.ringid, target, key.current);
        toast({ type: 'success', title: `Ringing ${account ? label(account) : target}` });
        onDone();
      }
    } catch (error) {
      key.current = newIdempotencyKey();
      toast({ type: 'error', title: mode.kind === 'call' ? 'The call didn’t go through' : 'Couldn’t add them', description: errorMessage(error) });
    } finally {
      setBusy(null);
    }
  }

  const actionLabel = mode.kind === 'call' ? 'Call' : 'Add';
  const ActionIcon = mode.kind === 'call' ? Phone : UserPlus;
  const showLookup = ref && lookup.state !== 'idle' && 'ref' in lookup && lookup.ref === ref;

  return (
    <div className={styles.dialer}>
      <SearchField
        label="c:handle or si:handle"
        placeholder="c:alice or si:scout"
        value={text}
        onValueChange={setText}
        autoFocus
        autoComplete="off"
        autoCapitalize="none"
        spellCheck={false}
        onKeyDown={event => {
          if (event.key === 'Enter' && lookup.state === 'found' && lookup.ref === ref) void dial(lookup.result.account.id, lookup.result.account);
        }}
      />

      {showLookup ? (
        <section className={styles.result} aria-live="polite" aria-label="Lookup result">
          {lookup.state === 'loading' ? <Skeleton avatar lines={2} label="Looking them up" /> : null}
          {lookup.state === 'found' ? (
            <div className={styles.found}>
              <PersonAvatar account={lookup.result.account} size="lg" />
              <div className={styles.foundText}>
                <h3 className={styles.foundName}>{label(lookup.result.account)}</h3>
                <div className={styles.badges}>
                  <Pixel keepCase>{lookup.result.account.id}</Pixel>
                  <SiliconBadge account={lookup.result.account} />
                  <Badge size="sm" tone={lookup.result.on_ring ? 'success' : 'neutral'}>{lookup.result.on_ring ? 'ON RING' : 'NOT ON RING YET'}</Badge>
                </div>
              </div>
              <Button
                variant="primary"
                loading={busy === lookup.result.account.id}
                disabled={Boolean(busy) || exclude.has(lookup.result.account.uuid)}
                onClick={() => void dial(lookup.result.account.id, lookup.result.account)}
              >
                <ActionIcon size={16} strokeWidth={1.75} aria-hidden="true" />
                {actionLabel}
              </Button>
            </div>
          ) : null}
          {lookup.state === 'missing' ? <p className={styles.note}>No one on Silicon Accounts has the id <strong>{lookup.ref}</strong>.</p> : null}
          {lookup.state === 'error' ? <p className={styles.note}>{lookup.message}</p> : null}
        </section>
      ) : text.trim() && !ref && !recent?.length ? (
        <p className={styles.note}>Type a full id, like <strong>c:alice</strong> or <strong>si:scout</strong>.</p>
      ) : null}

      <section className={styles.recent} aria-label="Recent people">
        <Pixel as="h2">Recent</Pixel>
        {recent === null ? <Skeleton avatar lines={2} label="Loading recent people" /> : null}
        {recent && !recent.length ? <p className={styles.note}>People you call will show up here.</p> : null}
        {recent?.length ? (
          <ul className={styles.people}>
            {recent.filter(person => !exclude.has(person.account.uuid)).map(person => (
              <li key={person.account.uuid} className={styles.person}>
                <PersonAvatar account={person.account} size="md" />
                <div className={styles.personText}>
                  <span className={styles.personName}>
                    <span className={styles.ellipsis}>{label(person.account)}</span>
                    <SiliconBadge account={person.account} />
                  </span>
                  <Pixel keepCase>{person.account.id} · {when(person.last_call_at)}</Pixel>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-label={`${actionLabel} ${label(person.account)}`}
                  loading={busy === person.account.id}
                  disabled={Boolean(busy)}
                  onClick={() => void dial(person.account.id, person.account)}
                >
                  <ActionIcon size={16} strokeWidth={1.75} aria-hidden="true" />
                </Button>
              </li>
            ))}
          </ul>
        ) : null}
      </section>
    </div>
  );
}
