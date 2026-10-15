'use client';

import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';
import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeft, Phone, Trash2 } from 'lucide-react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { EmptyState } from '@teamofsilicons/silicon-ui/empty-state';
import { Skeleton } from '@teamofsilicons/silicon-ui/skeleton';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { api, errorMessage, newIdempotencyKey } from '@/lib/api';
import { duration, fullDate, label, voicemailReason, when } from '@/lib/format';
import type { Voicemail } from '@/lib/types';
import { AudioPlayer, InlinePlay } from './audio-player';
import { NameBlock, PersonAvatar, Pixel, SiliconBadge } from './identity';
import { PageHeader } from './page';
import { useRing, useRingStore } from './ring-context';
import { RingRings } from './ring-mark';
import details from './details.module.css';
import styles from './lists.module.css';

async function audioUrl(id: string) {
  const voicemail = await api.voicemail(id);
  return voicemail.audio_status === 'ready' ? voicemail.audio_url ?? null : null;
}

export function VoicemailScreen() {
  const id = useSearchParams().get('id');
  return id ? <VoicemailDetail id={id} /> : <VoicemailList />;
}

function VoicemailList() {
  const revision = useRing(state => state.revision.voicemails);
  const [items, setItems] = useState<Voicemail[] | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api.voicemails({ limit: 50 }).then(
      page => {
        if (cancelled) return;
        setItems(page.items);
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
      const page = await api.voicemails({ limit: 50, cursor });
      setItems(current => [...(current ?? []), ...page.items]);
      setCursor(page.next_cursor);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setLoadingMore(false);
    }
  }, [cursor]);

  return (
    <>
      <PageHeader title="Voicemail" />
      {error && !items ? <Alert tone="danger" title="Voicemail didn’t load">{error}</Alert> : null}
      {!items && !error ? <Skeleton avatar lines={3} label="Loading voicemail" /> : null}
      {items && !items.length ? (
        <div className={styles.empty}>
          <EmptyState
            icon={<RingRings size={28} className={styles.emptyMark} />}
            title="No voicemail"
            description="When someone can’t reach you, their message shows up here with a transcript."
          />
        </div>
      ) : null}
      {items?.length ? (
        <ul className={styles.group} aria-label="Voicemail">
          {items.map(voicemail => (
            <li key={voicemail.voicemail_id} className={styles.row}>
              <Link href={`/voicemail?id=${encodeURIComponent(voicemail.voicemail_id)}`} className={styles.rowLink}>
                <span className={styles.lead}>
                  <span className={voicemail.read ? styles.readDot : styles.unreadDot} aria-label={voicemail.read ? undefined : 'Unread'} />
                  <PersonAvatar account={voicemail.from} size="md" />
                </span>
                <span className={styles.rowText}>
                  <span className={styles.rowTitle}>
                    <span className={styles.ellipsis}>{label(voicemail.from)}</span>
                    <SiliconBadge account={voicemail.from} />
                  </span>
                  <span className={styles.meta}>
                    <span className={styles.metaText}>{voicemailReason(voicemail)}{voicemail.duration_ms ? ` · ${duration(voicemail.duration_ms)}` : ''}</span>
                  </span>
                  <span className={styles.preview}>
                    {voicemail.transcript_status === 'ready' ? voicemail.transcript || 'No words were spoken.' : voicemail.transcript_status === 'pending' ? 'Transcribing…' : 'No transcript.'}
                  </span>
                </span>
                <span className={styles.time}>{when(voicemail.created_at)}</span>
              </Link>
              {voicemail.audio_status === 'ready' ? (
                <InlinePlay label={`voicemail from ${label(voicemail.from)}`} load={() => audioUrl(voicemail.voicemail_id)} />
              ) : <span />}
            </li>
          ))}
        </ul>
      ) : null}
      {cursor ? (
        <div className={styles.more}>
          <Button variant="secondary" loading={loadingMore} onClick={() => void loadMore()}>Show older voicemail</Button>
        </div>
      ) : null}
    </>
  );
}

function VoicemailDetail({ id }: { id: string }) {
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const revision = useRing(state => state.revision.voicemails);
  const [voicemail, setVoicemail] = useState<Voicemail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<'call' | 'delete' | 'read' | null>(null);
  const key = useRef(newIdempotencyKey());
  const markedRead = useRef(false);

  useEffect(() => {
    let cancelled = false;
    api.voicemail(id).then(
      result => {
        if (cancelled) return;
        setVoicemail(result);
        if (!result.read && !markedRead.current) {
          markedRead.current = true;
          api.markVoicemail(id, true).then(updated => {
            if (!cancelled) setVoicemail(current => (current ? { ...updated, audio_url: current.audio_url } : updated));
            void store.refreshUnread();
          }, () => undefined);
        }
      },
      failure => { if (!cancelled) setError(errorMessage(failure)); },
    );
    return () => { cancelled = true; };
  }, [id, revision, store]);

  async function callBack() {
    if (!voicemail) return;
    setBusy('call');
    try {
      const call = await store.startCall(voicemail.from.id, key.current);
      router.push(`/call?id=${encodeURIComponent(call.ringid)}`);
    } catch (failure) {
      key.current = newIdempotencyKey();
      toast({ type: 'error', title: 'The call didn’t go through', description: errorMessage(failure) });
      setBusy(null);
    }
  }

  async function remove() {
    setBusy('delete');
    try {
      await api.deleteVoicemail(id);
      store.bumpVoicemails();
      toast({ type: 'success', title: 'Voicemail deleted' });
      router.replace('/voicemail');
    } catch (failure) {
      toast({ type: 'error', title: 'Couldn’t delete it', description: errorMessage(failure) });
      setBusy(null);
    }
  }

  async function markUnread() {
    setBusy('read');
    try {
      const updated = await api.markVoicemail(id, false);
      setVoicemail(current => (current ? { ...updated, audio_url: current.audio_url } : updated));
      store.bumpVoicemails();
      toast({ type: 'success', title: 'Marked as unread' });
    } catch (failure) {
      toast({ type: 'error', title: 'Couldn’t mark it unread', description: errorMessage(failure) });
    } finally {
      setBusy(null);
    }
  }

  return (
    <article className={details.page}>
      <div className={details.back}>
        <ButtonLink href="/voicemail" as={Link} variant="ghost" size="sm">
          <ArrowLeft size={16} strokeWidth={1.75} aria-hidden="true" />
          Voicemail
        </ButtonLink>
      </div>
      {error ? <Alert tone="danger" title="This voicemail didn’t load">{error}</Alert> : null}
      {!voicemail && !error ? <Skeleton avatar lines={4} label="Loading voicemail" /> : null}
      {voicemail ? (
        <>
          <header className={details.hero}>
            <PersonAvatar account={voicemail.from} size="xl" />
            <NameBlock account={voicemail.from} size="title" />
            <Pixel tone={voicemail.reason === 'missed' ? 'danger' : 'foreground'}>{voicemailReason(voicemail)}</Pixel>
            <Pixel>{fullDate(voicemail.created_at)}</Pixel>
          </header>

          <section className={details.card} aria-label="Recording">
            {voicemail.audio_status === 'ready' ? (
              <AudioPlayer label="Voicemail" knownDurationMs={voicemail.duration_ms} load={() => audioUrl(voicemail.voicemail_id)} />
            ) : (
              <Pixel>{voicemail.audio_status === 'failed' ? 'The recording failed' : 'Processing the recording…'}</Pixel>
            )}
          </section>

          <section className={details.card} aria-label="Transcript">
            <Pixel as="h2">Transcript</Pixel>
            <p className={details.transcriptText}>
              {voicemail.transcript_status === 'ready'
                ? voicemail.transcript || 'No words were spoken.'
                : voicemail.transcript_status === 'pending' ? 'Transcribing…' : 'This voicemail couldn’t be transcribed.'}
            </p>
          </section>

          <div className={details.actions}>
            <Button variant="primary" loading={busy === 'call'} disabled={Boolean(busy)} onClick={() => void callBack()}>
              <Phone size={16} strokeWidth={1.75} aria-hidden="true" />
              Call back
            </Button>
            <Button variant="secondary" loading={busy === 'read'} disabled={Boolean(busy)} onClick={() => void markUnread()}>Mark as unread</Button>
            <ButtonLink href={`/history?id=${encodeURIComponent(voicemail.ringid)}`} as={Link} variant="ghost">Call details</ButtonLink>
            <Button variant="danger" loading={busy === 'delete'} disabled={Boolean(busy)} onClick={() => void remove()}>
              <Trash2 size={16} strokeWidth={1.75} aria-hidden="true" />
              Delete
            </Button>
          </div>
        </>
      ) : null}
    </article>
  );
}
