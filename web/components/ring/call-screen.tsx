'use client';

import Link from 'next/link';
import { useRouter, useSearchParams } from 'next/navigation';
import { useEffect, useRef, useState } from 'react';
import { Mic, MicOff, Phone, PhoneOff, UserPlus, Volume2 } from 'lucide-react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { EmptyState } from '@teamofsilicons/silicon-ui/empty-state';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { ApiError, errorMessage, newIdempotencyKey } from '@/lib/api';
import { callDuration, deviceLabel, duration, label, myInvitation, myParticipant, otherPeople, outcome, outcomeLabel } from '@/lib/format';
import type { MediaState } from '@/lib/store';
import type { Account, Call, Participant } from '@/lib/types';
import { useDialer } from './dialer';
import { CallControl } from './exceptions/call-control';
import { useMe, useNow } from './hooks';
import { NameBlock, PersonAvatar, Pixel, SiliconBadge } from './identity';
import { LiveTranscriptView } from './live-transcript';
import { useRing, useRingStore } from './ring-context';
import { RingRings } from './ring-mark';
import { keepClearOfToasts } from './toasts';
import styles from './call.module.css';

export function CallScreen() {
  const params = useSearchParams();
  const ringid = params.get('id') ?? '';
  const action = params.get('action');
  const store = useRingStore();
  const call = useRing(state => (ringid ? state.calls[ringid] : undefined));
  const [missing, setMissing] = useState<string | null>(null);

  useEffect(() => {
    if (!ringid) return;
    store.ensureCall(ringid).catch(error => setMissing(error instanceof ApiError && (error.status === 404 || error.status === 403) ? 'This call isn’t yours, or it no longer exists.' : errorMessage(error)));
  }, [ringid, store]);

  if (!ringid || missing) {
    return (
      <div className={styles.screen}>
        <EmptyState
          icon={<RingRings size={28} />}
          title="No call here"
          description={missing ?? 'Open a call from Recents or start a new one.'}
          action={<ButtonLink href="/" as={Link} variant="secondary">Back to Recents</ButtonLink>}
        />
      </div>
    );
  }
  if (!call) {
    return (
      <div className={styles.screen} aria-busy="true">
        <div className={styles.stage}><Pixel>Connecting…</Pixel></div>
      </div>
    );
  }
  return <CallView call={call} action={action} />;
}

function CallView({ call, action }: { call: Call; action: string | null }) {
  const me = useMe();
  const media = useRing(state => (state.media?.ringid === call.ringid ? state.media : null));
  const meUuid = me?.account.uuid;
  const mine = myParticipant(call, meUuid);
  const invitation = myInvitation(call, meUuid);

  if (invitation?.state === 'ringing' && mine?.state !== 'joined') return <IncomingFromLink call={call} action={action} />;
  if (call.state === 'ended' || (mine?.state === 'left' && !media)) return <EndedView call={call} />;
  if (call.state === 'voicemail' && call.created_by.uuid === meUuid) return <VoicemailView call={call} media={media} />;
  if (call.state === 'ringing' && call.created_by.uuid === meUuid) return <OutgoingView call={call} media={media} />;
  if (mine?.state === 'joined') return <InCallView call={call} media={media} />;
  return <EndedView call={call} />;
}

/** Ends the call (or cancels it while it is still ringing). */
function useHangUp(call: Call) {
  const store = useRingStore();
  const { toast } = useToastStack();
  const [busy, setBusy] = useState(false);
  const hangUp = async () => {
    setBusy(true);
    try {
      await store.leave(call.ringid);
    } catch (error) {
      toast({ type: 'error', title: 'Couldn’t hang up', description: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  };
  return { hangUp, busy };
}

function Halo({ account, waves, speaking, recording, size = 'xl' }: { account: Account; waves?: boolean; speaking?: boolean; recording?: boolean; size?: 'lg' | 'xl' }) {
  const classes = [styles.halo, waves ? styles.waves : '', speaking ? styles.speaking : '', recording ? styles.recording : ''].filter(Boolean).join(' ');
  return (
    <span className={classes}>
      <PersonAvatar account={account} size={size} />
    </span>
  );
}

/**
 * Shown while this browser carries no audio for a call I'm on: where the call went (another device), or that it is
 * still on this browser after a reload, with the button that brings it here.
 */
function ConnectionBanner({ call, media }: { call: Call; media: MediaState | null }) {
  const store = useRingStore();
  const me = useMe();
  const movedTo = useRing(state => state.movedTo[call.ringid]);
  const resume = useRing(state => (state.resume?.ringid === call.ringid ? state.resume : null));
  const { toast } = useToastStack();
  const [busy, setBusy] = useState(false);
  // A room that failed to connect (or dropped for good) leaves media in place but disconnected: offer it again too.
  if (media && media.status !== 'disconnected') return null;
  const device = myParticipant(call, me?.account.uuid)?.device;
  const switching = Boolean(movedTo || (!media && device && device.device_id !== me?.device.device_id));

  async function bringHere() {
    setBusy(true);
    try {
      if (switching) await store.switchHere(call.ringid);
      else await store.resumeCall(call.ringid);
    } catch (error) {
      toast({ type: 'error', title: switching ? 'Couldn’t switch the call' : 'Couldn’t rejoin the call', description: errorMessage(error) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className={styles.handoff} role="status">
      <span>
        {movedTo ? `Moved to ${movedTo}`
          : switching ? `Active on ${deviceLabel(device)}`
            : resume?.state === 'joining' ? 'Rejoining the call…' : 'This browser isn’t connected to the call'}
      </span>
      <Button size="sm" variant={switching ? 'secondary' : 'primary'} loading={busy || resume?.state === 'joining'} onClick={() => void bringHere()}>
        {switching ? 'Switch to this browser' : 'Rejoin call'}
      </Button>
    </div>
  );
}

function MediaNotes({ media }: { media: MediaState | null }) {
  const store = useRingStore();
  if (!media) return null;
  return (
    <div className={styles.notes}>
      {media.micError ? <Alert tone="warning" title="Microphone unavailable">{media.micError}</Alert> : null}
      {media.audioBlocked ? (
        <Button variant="secondary" onClick={() => void store.startAudio()}>
          <Volume2 size={16} strokeWidth={1.75} aria-hidden="true" />
          Turn on sound
        </Button>
      ) : null}
    </div>
  );
}

function OutgoingView({ call, media }: { call: Call; media: MediaState | null }) {
  const me = useMe();
  const { hangUp, busy } = useHangUp(call);
  const callee = otherPeople(call, me?.account.uuid)[0] ?? call.created_by;
  return (
    <div className={styles.screen}>
      <ConnectionBanner call={call} media={media} />
      <div className={styles.stage}>
        <Halo account={callee} waves />
        <NameBlock account={callee} />
        <Pixel tone="foreground" className={styles.callStatus}>{media?.status === 'connecting' ? 'Calling…' : 'Ringing'}</Pixel>
        <MediaNotes media={media} />
      </div>
      <div className={styles.controls} ref={keepClearOfToasts}>
        <CallControl tone="danger" label="Hang up" icon={<PhoneOff size={26} strokeWidth={1.75} aria-hidden="true" />} busy={busy} onClick={() => void hangUp()} />
      </div>
    </div>
  );
}

function VoicemailView({ call, media }: { call: Call; media: MediaState | null }) {
  const me = useMe();
  const { hangUp, busy } = useHangUp(call);
  const callee = otherPeople(call, me?.account.uuid)[0] ?? call.created_by;
  const now = useNow();
  const [since] = useState(now);
  const reason = call.invitations.find(invitation => invitation.to.uuid === callee.uuid);
  return (
    <div className={styles.screen}>
      <ConnectionBanner call={call} media={media} />
      <div className={styles.stage}>
        <Halo account={callee} recording />
        <NameBlock account={callee} />
        <div className={styles.status}>
          {reason?.state === 'declined' ? <Pixel>{reason.reason ? `Declined · ${reason.reason}` : 'Declined'}</Pixel> : null}
          {reason?.state === 'busy' ? <Pixel>Busy</Pixel> : null}
          <Pixel tone="foreground" className={styles.callStatus}>Leave a message after the beep</Pixel>
          <Pixel>{duration(now - since)}</Pixel>
        </div>
        <MediaNotes media={media} />
      </div>
      <div className={styles.controls} ref={keepClearOfToasts}>
        <Button variant="primary" size="lg" loading={busy} onClick={() => void hangUp()}>Done</Button>
      </div>
    </div>
  );
}

function ParticipantTile({ participant, speaking, meUuid, solo }: { participant: Participant; speaking: boolean; meUuid: string | undefined; solo: boolean }) {
  const account = participant.account;
  const representative = participant.representative;
  return (
    <li className={solo ? styles.soloTile : styles.tile}>
      <Halo account={account} speaking={speaking} size={solo ? 'xl' : 'lg'} />
      {solo ? (
        <NameBlock account={account} />
      ) : (
        <span className={styles.tileName}>
          <span className={styles.tileLabel}>{account.uuid === meUuid ? 'You' : label(account)}</span>
          <SiliconBadge account={account} />
        </span>
      )}
      {representative && representative.status !== 'live' ? (
        <Pixel tone={representative.status === 'failed' ? 'danger' : 'muted'}>
          {representative.status === 'starting' ? 'Joining…' : representative.status === 'failed' ? 'Voice unavailable' : 'Left'}
        </Pixel>
      ) : null}
    </li>
  );
}

function InCallView({ call, media }: { call: Call; media: MediaState | null }) {
  const store = useRingStore();
  const me = useMe();
  const openDialer = useDialer();
  const transcript = useRing(state => state.transcripts[call.ringid]);
  const rejoining = useRing(state => state.resume?.ringid === call.ringid && state.resume.state === 'joining');
  const { hangUp, busy } = useHangUp(call);
  const now = useNow();
  const meUuid = me?.account.uuid;
  const mine = myParticipant(call, meUuid);
  const here = Boolean(media) && media?.status !== 'disconnected';
  const elsewhere = !media && mine?.device && mine.device.device_id !== me?.device.device_id;
  const others = call.participants.filter(p => p.state === 'joined' && p.account.uuid !== meUuid);
  const ringing = call.invitations.filter(invitation => invitation.state === 'ringing' && invitation.to.uuid !== meUuid);
  const speaking = new Set(media?.speaking ?? []);
  const solo = others.length === 1;

  useEffect(() => {
    void store.syncTranscript(call.ringid);
  }, [call.ringid, store]);

  const timer = call.answered_at ? duration(now - Date.parse(call.answered_at)) : '0:00';
  const status = media?.status === 'reconnecting' || media?.status === 'connecting' ? 'Connecting…' : 'On call';

  return (
    <div className={[styles.screen, styles.inCall].join(' ')}>
      <ConnectionBanner call={call} media={media} />

      <header className={styles.callHeader}>
        <Pixel tone="foreground" className={styles.timer}>{timer}</Pixel>
        <Pixel>{here ? status : elsewhere ? `On ${deviceLabel(mine?.device)}` : rejoining ? 'Connecting…' : 'Not connected'}</Pixel>
      </header>

      <ul className={solo ? styles.soloRoster : styles.roster} aria-label="On the call">
        {others.map(participant => (
          <ParticipantTile
            key={participant.account.uuid}
            participant={participant}
            speaking={Boolean(participant.media_identity && speaking.has(participant.media_identity))}
            meUuid={meUuid}
            solo={solo}
          />
        ))}
        {!others.length ? <li className={styles.waiting}><Pixel>Waiting for others…</Pixel></li> : null}
      </ul>
      {ringing.length ? <Pixel className={styles.ringingLine}>Ringing {ringing.map(invitation => label(invitation.to)).join(', ')}</Pixel> : null}

      <MediaNotes media={media} />
      <LiveTranscriptView transcript={transcript} meUuid={meUuid} />

      <div className={styles.controls} ref={keepClearOfToasts}>
        <CallControl
          label={media?.muted ? 'Unmute' : 'Mute'}
          icon={media?.muted ? <MicOff size={24} strokeWidth={1.75} aria-hidden="true" /> : <Mic size={24} strokeWidth={1.75} aria-hidden="true" />}
          pressed={Boolean(media?.muted)}
          size="md"
          disabled={!here}
          onClick={() => void store.setMuted(!media?.muted)}
        />
        <CallControl
          label="Add"
          icon={<UserPlus size={24} strokeWidth={1.75} aria-hidden="true" />}
          size="md"
          onClick={() => openDialer({
            kind: 'invite',
            ringid: call.ringid,
            exclude: [...call.participants.filter(p => p.state === 'joined').map(p => p.account.uuid), ...ringing.map(i => i.to.uuid)],
          })}
        />
        <CallControl tone="danger" label="Hang up" icon={<PhoneOff size={26} strokeWidth={1.75} aria-hidden="true" />} busy={busy} onClick={() => void hangUp()} />
      </div>
    </div>
  );
}

function EndedView({ call }: { call: Call }) {
  const me = useMe();
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const [calling, setCalling] = useState(false);
  const key = useRef(newIdempotencyKey());
  const meUuid = me?.account.uuid;
  const person = otherPeople(call, meUuid)[0] ?? call.created_by;
  const result = outcome(call, meUuid);
  const length = callDuration(call);
  const declined = call.invitations.find(invitation => invitation.state === 'declined' && invitation.to.uuid !== meUuid);
  const title = call.state === 'ended' ? 'Call ended'
    : myParticipant(call, meUuid)?.state === 'left' ? 'You left the call'
      : myInvitation(call, meUuid)?.state === 'declined' ? 'You declined' : 'Not on this call';

  async function callAgain() {
    setCalling(true);
    try {
      const next = await store.startCall(person.id, key.current);
      router.replace(`/call?id=${encodeURIComponent(next.ringid)}`);
    } catch (error) {
      key.current = newIdempotencyKey();
      toast({ type: 'error', title: 'The call didn’t go through', description: errorMessage(error) });
      setCalling(false);
    }
  }

  return (
    <div className={styles.screen}>
      <div className={styles.stage}>
        <Halo account={person} />
        <NameBlock account={person} />
        <div className={styles.status}>
          <Pixel tone={result === 'missed' || result === 'declined' || result === 'failed' ? 'danger' : 'foreground'} className={styles.callStatus}>
            {title}
          </Pixel>
          <Pixel>
            {[outcomeLabel(result, call.direction), length ? duration(length) : null, declined?.reason].filter(Boolean).join(' · ')}
          </Pixel>
        </div>
      </div>
      <div className={styles.endActions} ref={keepClearOfToasts}>
        <ButtonLink href={`/history?id=${encodeURIComponent(call.ringid)}`} as={Link} variant="secondary">Call details</ButtonLink>
        {person.uuid !== meUuid ? <Button variant="primary" loading={calling} onClick={() => void callAgain()}>Call again</Button> : null}
        <ButtonLink href="/" as={Link} variant="ghost">Back to Recents</ButtonLink>
      </div>
    </div>
  );
}

/**
 * /call?id=… for a call ringing for me. With action=answer|decline (a notification's buttons) it does that once.
 * Otherwise the ringing sheet covers it, except when this browser doesn't ring (silenced, or ringing turned off here),
 * so the screen keeps its own Accept and Decline.
 */
function IncomingFromLink({ call, action }: { call: Call; action: string | null }) {
  const store = useRingStore();
  const me = useMe();
  const router = useRouter();
  const { toast } = useToastStack();
  const done = useRef(false);
  const [busy, setBusy] = useState<'accept' | 'decline' | null>(null);
  const invitation = myInvitation(call, me?.account.uuid);
  const caller = invitation?.from ?? call.created_by;
  const pending = action === 'answer' || action === 'decline';

  async function respond(kind: 'accept' | 'decline') {
    if (!invitation) return;
    setBusy(kind);
    try {
      if (kind === 'accept') await store.accept(call.ringid, invitation.invitation_id);
      else await store.decline(call.ringid, invitation.invitation_id);
      if (kind === 'decline') router.replace('/');
    } catch (error) {
      toast({ type: 'error', title: kind === 'accept' ? 'Couldn’t answer' : 'Couldn’t decline', description: errorMessage(error) });
      setBusy(null);
    }
  }

  useEffect(() => {
    if (done.current || !invitation || (action !== 'answer' && action !== 'decline')) return;
    done.current = true;
    const ringid = call.ringid;
    const run = action === 'answer' ? store.accept(ringid, invitation.invitation_id) : store.decline(ringid, invitation.invitation_id);
    run.then(
      () => router.replace(action === 'answer' ? `/call?id=${encodeURIComponent(ringid)}` : '/'),
      error => {
        toast({ type: 'error', title: action === 'answer' ? 'Couldn’t answer' : 'Couldn’t decline', description: errorMessage(error) });
        router.replace(`/call?id=${encodeURIComponent(ringid)}`);
      },
    );
  }, [action, call.ringid, invitation, router, store, toast]);

  return (
    <div className={styles.screen} aria-busy={pending || undefined}>
      <div className={styles.stage}>
        <Halo account={caller} waves />
        <NameBlock account={caller} />
        <Pixel tone="foreground" className={styles.callStatus}>{action === 'answer' ? 'Answering…' : action === 'decline' ? 'Declining…' : 'Is calling'}</Pixel>
      </div>
      {pending || !invitation ? null : (
        <div className={styles.controls} ref={keepClearOfToasts}>
          <CallControl
            tone="danger"
            label="Decline"
            icon={<PhoneOff size={26} strokeWidth={1.75} aria-hidden="true" />}
            busy={busy === 'decline'}
            disabled={busy === 'accept'}
            onClick={() => void respond('decline')}
          />
          <CallControl
            tone="accept"
            label="Accept"
            icon={<Phone size={26} strokeWidth={1.75} aria-hidden="true" />}
            busy={busy === 'accept'}
            disabled={busy === 'decline'}
            onClick={() => void respond('accept')}
          />
        </div>
      )}
    </div>
  );
}
