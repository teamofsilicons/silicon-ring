'use client';

import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';
import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, Ban, Flag, Phone, PhoneIncoming, PhoneMissed, PhoneOff, UserCheck, UserPlus, Voicemail as VoicemailIcon } from 'lucide-react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { DropdownMenu } from '@teamofsilicons/silicon-ui/dropdown-menu';
import { EmptyState } from '@teamofsilicons/silicon-ui/empty-state';
import { Skeleton } from '@teamofsilicons/silicon-ui/skeleton';
import { Timeline, type TimelineEvent } from '@teamofsilicons/silicon-ui/timeline';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { api, errorMessage, fullTranscript, newIdempotencyKey, reportLink } from '@/lib/api';
import { callDuration, clock, duration, fullDate, label, otherPeople, outcome, outcomeLabel, spokenDuration } from '@/lib/format';
import type { Account, Call, Recording, TranscriptEntry } from '@/lib/types';
import { AudioPlayer } from './audio-player';
import { useBlocks, useMe } from './hooks';
import { NameBlock, PersonAvatar, Pixel, SiliconBadge } from './identity';
import { useRing, useRingStore } from './ring-context';
import { RingRings } from './ring-mark';
import styles from './details.module.css';
import lists from './lists.module.css';

export function CallDetailsScreen() {
  const id = useSearchParams().get('id');
  if (!id) {
    return (
      <EmptyState
        icon={<RingRings size={28} />}
        title="No call selected"
        description="Pick a call from Recents to see who was on it and what was said."
        action={<ButtonLink href="/" as={Link} variant="secondary">Recents</ButtonLink>}
      />
    );
  }
  return <CallDetails ringid={id} />;
}

function timelineEvents(call: Call, entries: TranscriptEntry[]): TimelineEvent[] {
  const events: TimelineEvent[] = [];
  const icon = (Icon: typeof Phone) => <Icon size={14} strokeWidth={1.75} aria-hidden="true" />;
  for (const entry of entries) {
    const actor = entry.actor ? label(entry.actor) : undefined;
    const base = { id: String(entry.seq), at: entry.at, actor, avatar: entry.actor?.pfp_url ?? undefined };
    switch (entry.kind) {
      case 'call_started':
        events.push({ ...base, title: 'started the call', icon: icon(Phone) });
        break;
      case 'answered': {
        const after = typeof entry.data.after_ms === 'number' ? spokenDuration(entry.data.after_ms) : '';
        events.push({ ...base, title: after ? `answered after ${after}` : 'answered', tone: 'success', icon: icon(PhoneIncoming) });
        break;
      }
      case 'joined':
        events.push({ ...base, title: 'joined' });
        break;
      case 'left':
        events.push({ ...base, title: 'left' });
        break;
      case 'invited': {
        const invitee = entry.data.invitee as { display_name?: string | null; id?: string } | undefined;
        events.push({ ...base, title: `invited ${invitee?.display_name || invitee?.id || 'someone'}`, icon: icon(UserPlus) });
        break;
      }
      case 'declined':
        events.push({ ...base, title: entry.data.reason ? `declined: “${String(entry.data.reason)}”` : 'declined', tone: 'danger', icon: icon(PhoneOff) });
        break;
      case 'missed':
        events.push({ ...base, title: actor ? 'missed the call' : 'Missed', tone: 'danger', icon: icon(PhoneMissed) });
        break;
      case 'busy':
        events.push({ ...base, title: actor ? 'was busy' : 'Busy', tone: 'danger' });
        break;
      case 'voicemail':
        events.push({ ...base, title: actor ? 'left a voicemail' : 'Voicemail left', icon: icon(VoicemailIcon) });
        break;
      case 'call_ended':
        events.push({ ...base, actor: undefined, title: `Call ended${entry.data.reason ? ` (${String(entry.data.reason).replace(/_/g, ' ')})` : ''}`, icon: icon(PhoneOff) });
        break;
      case 'note':
        if (entry.text) events.push({ ...base, title: entry.text });
        break;
      default:
        break;
    }
  }
  if (!events.length) events.push({ id: 'created', at: call.created_at, actor: label(call.created_by), title: 'started the call', icon: icon(Phone) });
  return events;
}

function CallDetails({ ringid }: { ringid: string }) {
  const me = useMe();
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const blocks = useBlocks();
  const revision = useRing(state => state.revision.calls);
  const [call, setCall] = useState<Call | null>(null);
  const [entries, setEntries] = useState<TranscriptEntry[] | null>(null);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [calling, setCalling] = useState(false);
  const key = useRef(newIdempotencyKey());
  const [now] = useState(() => Date.now());
  const meUuid = me?.account.uuid;

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.call(ringid), fullTranscript(ringid).catch(() => [] as TranscriptEntry[])]).then(
      ([result, transcript]) => {
        if (cancelled) return;
        setCall(result);
        setEntries(transcript);
        if (result.recording.status !== 'none') api.recording(ringid).then(value => { if (!cancelled) setRecording(value); }, () => undefined);
      },
      failure => { if (!cancelled) setError(errorMessage(failure)); },
    );
    return () => { cancelled = true; };
  }, [ringid, revision]);

  const events = useMemo(() => (call && entries ? timelineEvents(call, entries) : []), [call, entries]);
  const speech = useMemo(() => (entries ?? []).filter(entry => entry.kind === 'speech' && entry.text?.trim()), [entries]);

  if (error) {
    return (
      <article className={styles.page}>
        <Back />
        <Alert tone="danger" title="This call didn’t load">{error}</Alert>
      </article>
    );
  }
  if (!call || !entries) {
    return (
      <article className={styles.page}>
        <Back />
        <Skeleton avatar lines={4} label="Loading the call" />
      </article>
    );
  }

  const people = otherPeople(call, meUuid);
  const person = people[0] ?? call.created_by;
  const result = outcome(call, meUuid);
  const length = callDuration(call);
  const live = call.state !== 'ended';

  async function callBack() {
    setCalling(true);
    try {
      const next = await store.startCall(person.id, key.current);
      router.push(`/call?id=${encodeURIComponent(next.ringid)}`);
    } catch (failure) {
      key.current = newIdempotencyKey();
      toast({ type: 'error', title: 'The call didn’t go through', description: errorMessage(failure) });
      setCalling(false);
    }
  }

  return (
    <article className={styles.page}>
      <Back />
      <header className={styles.hero}>
        <PersonAvatar account={person} size="xl" />
        <NameBlock account={person} size="title" />
        {people.length > 1 ? <Pixel>With {people.slice(1).map(label).join(', ')}</Pixel> : null}
        <Pixel tone={result === 'missed' || result === 'declined' || result === 'failed' ? 'danger' : 'foreground'}>
          {[outcomeLabel(result, call.direction), length ? duration(length) : null].filter(Boolean).join(' · ')}
        </Pixel>
        <Pixel>{fullDate(call.created_at)}</Pixel>
      </header>

      <div className={styles.actions}>
        {live ? (
          <ButtonLink href={`/call?id=${encodeURIComponent(call.ringid)}`} as={Link} variant="primary">Open the call</ButtonLink>
        ) : (
          <Button variant="primary" loading={calling} onClick={() => void callBack()}>
            <Phone size={16} strokeWidth={1.75} aria-hidden="true" />
            Call {person.kind === 'silicon' || !person.display_name ? person.id : person.display_name.split(/\s+/)[0]}
          </Button>
        )}
        {call.voicemail_id ? <ButtonLink href={`/voicemail?id=${encodeURIComponent(call.voicemail_id)}`} as={Link} variant="secondary">Voicemail</ButtonLink> : null}
      </div>

      <section className={styles.card} aria-label="People">
        <Pixel as="h2">People</Pixel>
        <ul className={lists.group}>
          {call.participants.map(participant => (
            <li key={participant.account.uuid} className={lists.row}>
              <span className={lists.rowStatic}>
                <PersonAvatar account={participant.account} size="md" />
                <span className={lists.rowText}>
                  <span className={lists.rowTitle}>
                    <span className={lists.ellipsis}>{participant.account.uuid === meUuid ? `${label(participant.account)} (you)` : label(participant.account)}</span>
                    <SiliconBadge account={participant.account} />
                  </span>
                  <span className={lists.meta}>
                    <span className={lists.metaText}>
                      {[
                        `Joined ${clock(participant.joined_at)}`,
                        participant.left_at ? `left ${clock(participant.left_at)}` : participant.state === 'joined' ? 'on the call' : null,
                        participant.invited_by ? `invited by ${label(participant.invited_by)}` : null,
                      ].filter(Boolean).join(' · ')}
                    </span>
                  </span>
                </span>
                <span />
              </span>
              {participant.account.uuid === meUuid ? null : <PersonActions account={participant.account} ringid={call.ringid} blocks={blocks} />}
            </li>
          ))}
          {call.invitations.filter(invitation => invitation.state !== 'accepted').map(invitation => (
            <li key={invitation.invitation_id} className={lists.row}>
              <span className={lists.rowStatic}>
                <PersonAvatar account={invitation.to} size="md" />
                <span className={lists.rowText}>
                  <span className={lists.rowTitle}>
                    <span className={lists.ellipsis}>{invitation.to.uuid === meUuid ? `${label(invitation.to)} (you)` : label(invitation.to)}</span>
                    <SiliconBadge account={invitation.to} />
                  </span>
                  <span className={[lists.meta, invitation.state === 'ringing' ? '' : lists.missed].join(' ')}>
                    <span className={lists.metaText}>{[invitation.state, invitation.reason].filter(Boolean).join(' · ')}</span>
                  </span>
                </span>
                <span />
              </span>
              {invitation.to.uuid === meUuid ? null : <PersonActions account={invitation.to} ringid={call.ringid} blocks={blocks} />}
            </li>
          ))}
        </ul>
      </section>

      <section className={styles.card} aria-label="Recording">
        <Pixel as="h2">Recording</Pixel>
        {recording?.status === 'ready' ? (
          <AudioPlayer
            label="Recording"
            knownDurationMs={recording.duration_ms}
            load={async () => (await api.recording(ringid)).url ?? null}
          />
        ) : (
          <p className={styles.muted}>
            {call.recording.status === 'none' ? 'Nothing was recorded: the call never connected.'
              : call.recording.status === 'failed' || recording?.status === 'failed' ? 'The recording failed.'
                : call.recording.status === 'recording' ? 'Recording while the call is live.'
                  : 'Processing the recording…'}
          </p>
        )}
      </section>

      <section className={styles.card} aria-label="Timeline">
        <Pixel as="h2">Timeline</Pixel>
        <Timeline events={events} now={now} label="What happened on this call" headingLevel={3} />
      </section>

      <section className={styles.card} aria-label="Transcript">
        <Pixel as="h2">Transcript</Pixel>
        {speech.length ? (
          <ol className={styles.transcript}>
            {speech.map(entry => (
              <li key={entry.seq} className={styles.transcriptLine}>
                <span className={styles.transcriptMeta}>
                  <Pixel tone={entry.actor?.uuid === meUuid ? 'accent' : 'muted'}>{entry.actor ? label(entry.actor) : 'Someone'}</Pixel>
                  {entry.offset_ms !== null ? <Pixel>{duration(entry.offset_ms)}</Pixel> : null}
                </span>
                <span className={styles.transcriptText}>{entry.text}</span>
              </li>
            ))}
          </ol>
        ) : (
          <p className={styles.muted}>{call.answered_at ? 'Nothing was said yet.' : 'The call was never answered, so there is no transcript.'}</p>
        )}
      </section>
    </article>
  );
}

/**
 * Block (or Unblock) and Report for someone on the call. A blocked account's calls and invitations end unanswered
 * without ringing any of your devices, and it isn't told. Reporting opens an email to Team of Silicons.
 */
function PersonActions({ account, ringid, blocks }: { account: Account; ringid: string; blocks: ReturnType<typeof useBlocks> }) {
  const { toast } = useToastStack();
  const name = label(account);
  const blocked = blocks.isBlocked(account);

  async function block() {
    try {
      await blocks.block(account);
      toast({
        type: 'success',
        title: `Blocked ${name}`,
        description: 'Their calls and invitations won’t ring for you. They aren’t told.',
        action: { label: 'Undo', onClick: () => void unblock() },
      });
    } catch (failure) {
      toast({ type: 'error', title: `Couldn’t block ${name}`, description: errorMessage(failure) });
    }
  }

  async function unblock() {
    try {
      await blocks.unblock(account);
      toast({ type: 'success', title: `Unblocked ${name}` });
    } catch (failure) {
      toast({ type: 'error', title: `Couldn’t unblock ${name}`, description: errorMessage(failure) });
    }
  }

  return (
    <DropdownMenu
      label="More"
      items={[
        blocked
          ? { label: `Unblock ${name}`, icon: <UserCheck size={16} strokeWidth={1.75} />, onSelect: () => void unblock() }
          : { label: `Block ${name}`, icon: <Ban size={16} strokeWidth={1.75} />, destructive: true, onSelect: () => void block() },
        { label: `Report ${name}`, icon: <Flag size={16} strokeWidth={1.75} />, onSelect: () => window.location.assign(reportLink(account, ringid)) },
      ]}
    />
  );
}

function Back() {
  return (
    <div className={styles.back}>
      <ButtonLink href="/" as={Link} variant="ghost" size="sm">
        <ArrowLeft size={16} strokeWidth={1.75} aria-hidden="true" />
        Recents
      </ButtonLink>
    </div>
  );
}
