'use client';

import Link from 'next/link';
import { useRouter } from 'next/navigation';
import { useCallback, useEffect, useRef, useState } from 'react';
import { Phone, PhoneIncoming, PhoneMissed, PhoneOutgoing, Voicemail as VoicemailIcon } from 'lucide-react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { EmptyState } from '@teamofsilicons/silicon-ui/empty-state';
import { Skeleton } from '@teamofsilicons/silicon-ui/skeleton';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { api, errorMessage, newIdempotencyKey } from '@/lib/api';
import { callDuration, duration, label, otherPeople, outcome, outcomeLabel, when } from '@/lib/format';
import type { Call } from '@/lib/types';
import { useDialer } from './dialer';
import { useMe } from './hooks';
import { PersonAvatar, SiliconBadge } from './identity';
import { NotificationsPrompt } from './notifications-prompt';
import { PageHeader } from './page';
import { useRing, useRingStore } from './ring-context';
import { RingRings } from './ring-mark';
import styles from './lists.module.css';

export function Recents() {
  const revision = useRing(state => state.revision.calls);
  const [calls, setCalls] = useState<Call[] | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const openDialer = useDialer();

  useEffect(() => {
    let cancelled = false;
    api.calls({ direction: 'all', limit: 50 }).then(
      page => {
        if (cancelled) return;
        setCalls(page.items);
        setCursor(page.next_cursor);
        setError(null);
      },
      failure => { if (!cancelled) setError(errorMessage(failure)); },
    );
    return () => { cancelled = true; };
  }, [revision]);

  const loadMore = useCallback(async () => {
    if (!cursor) return;
    setLoadingMore(true);
    try {
      const page = await api.calls({ direction: 'all', limit: 50, cursor });
      setCalls(current => {
        const seen = new Set((current ?? []).map(call => call.ringid));
        return [...(current ?? []), ...page.items.filter(call => !seen.has(call.ringid))];
      });
      setCursor(page.next_cursor);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setLoadingMore(false);
    }
  }, [cursor]);

  return (
    <>
      <PageHeader title="Recents" />
      <NotificationsPrompt />
      {error && !calls ? <Alert tone="danger" title="Recents didn’t load">{error}</Alert> : null}
      {!calls && !error ? <Skeleton avatar lines={2} label="Loading recents" /> : null}
      {calls && !calls.length ? (
        <div className={styles.empty}>
          <EmptyState
            icon={<RingRings size={28} className={styles.emptyMark} />}
            title="No calls yet"
            description="Calls you make and receive show up here."
            action={<Button variant="primary" onClick={() => openDialer()}>New call</Button>}
          />
        </div>
      ) : null}
      {calls?.length ? (
        <ul className={styles.group} aria-label="Recent calls">
          {calls.map(call => <CallRow key={call.ringid} call={call} />)}
        </ul>
      ) : null}
      {cursor ? (
        <div className={styles.more}>
          <Button variant="secondary" loading={loadingMore} onClick={() => void loadMore()}>Show older calls</Button>
        </div>
      ) : null}
    </>
  );
}

function DirectionIcon({ call, result }: { call: Call; result: ReturnType<typeof outcome> }) {
  const props = { size: 14, strokeWidth: 1.75, 'aria-hidden': true } as const;
  if (result === 'missed' && call.direction === 'incoming') return <PhoneMissed {...props} />;
  if (result === 'voicemail') return <VoicemailIcon {...props} />;
  return call.direction === 'incoming' ? <PhoneIncoming {...props} /> : <PhoneOutgoing {...props} />;
}

function CallRow({ call }: { call: Call }) {
  const me = useMe();
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const [calling, setCalling] = useState(false);
  const key = useRef(newIdempotencyKey());
  const people = otherPeople(call, me?.account.uuid);
  const person = people[0] ?? call.created_by;
  const result = outcome(call, me?.account.uuid);
  const missed = result === 'missed' && call.direction === 'incoming';
  const length = callDuration(call);
  const meta = [outcomeLabel(result, call.direction), length && result === 'answered' ? duration(length) : null].filter(Boolean).join(' · ');
  const name = people.length > 1 ? `${label(person)} +${people.length - 1}` : label(person);

  async function redial() {
    setCalling(true);
    try {
      const next = await store.startCall(person.id, key.current);
      router.push(`/call?id=${encodeURIComponent(next.ringid)}`);
    } catch (error) {
      key.current = newIdempotencyKey();
      toast({ type: 'error', title: 'The call didn’t go through', description: errorMessage(error) });
    } finally {
      setCalling(false);
    }
  }

  return (
    <li className={[styles.row, missed ? styles.missed : ''].join(' ')}>
      <Link href={`/history?id=${encodeURIComponent(call.ringid)}`} className={styles.rowLink}>
        <PersonAvatar account={person} size="md" />
        <span className={styles.rowText}>
          <span className={styles.rowTitle}>
            <span className={styles.ellipsis}>{name}</span>
            <SiliconBadge account={person} />
          </span>
          <span className={styles.meta}>
            <DirectionIcon call={call} result={result} />
            <span className={styles.metaText}>{meta}</span>
          </span>
        </span>
        <span className={styles.time}>{when(call.created_at)}</span>
      </Link>
      <Button variant="ghost" size="sm" aria-label={`Call ${label(person)}`} loading={calling} onClick={() => void redial()}>
        <Phone size={16} strokeWidth={1.75} aria-hidden="true" />
      </Button>
    </li>
  );
}
