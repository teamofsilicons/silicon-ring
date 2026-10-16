'use client';

import { usePathname, useRouter, useSearchParams } from 'next/navigation';
import { Suspense, useEffect, useRef, useState } from 'react';
import { Phone, PhoneOff, Send } from 'lucide-react';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { Input } from '@teamofsilicons/silicon-ui/input';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { ApiError, errorMessage } from '@/lib/api';
import { codePoints, label } from '@/lib/format';
import type { IncomingRing } from '@/lib/store';
import { CallControl } from './exceptions/call-control';
import { useIncomingRings } from './hooks';
import { NameBlock, PersonAvatar, Pixel } from './identity';
import { useRing, useRingStore } from './ring-context';
import { keepClearOfToasts } from './toasts';
import styles from './call.module.css';

const REASONS = ['Can’t talk right now', 'In a meeting', 'Call you back soon', 'Text me instead'];

/** The full-screen ringing sheet, above whatever screen is open. Ink in both themes, in the Ring mark's colours. */
export function IncomingCallLayer() {
  return (
    <Suspense fallback={null}>
      <IncomingCallGate />
    </Suspense>
  );
}

function IncomingCallGate() {
  const rings = useIncomingRings();
  const pathname = usePathname();
  const params = useSearchParams();
  // A notification's Answer or Decline opens /call?action=…, which handles that ring itself.
  const action = params.get('action');
  const handledByPage = pathname.startsWith('/call') && (action === 'answer' || action === 'decline') && rings[0] && params.get('id') === rings[0].call.ringid;
  if (!rings.length || handledByPage) return null;
  return <IncomingCallSheet key={rings[0].invitation.invitation_id} ring={rings[0]} more={rings.length - 1} />;
}

function IncomingCallSheet({ ring, more }: { ring: IncomingRing; more: number }) {
  const store = useRingStore();
  const router = useRouter();
  const { toast } = useToastStack();
  const limit = useRing(state => state.meta?.limits.decline_reason ?? 200);
  const meUuid = useRing(state => state.me?.account.uuid);
  const [declining, setDeclining] = useState(false);
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState<'accept' | 'decline' | 'silence' | null>(null);
  const previousFocus = useRef<Element | null>(null);
  const { call, invitation } = ring;
  const caller = invitation.from;
  const others = call.participants.filter(p => p.state === 'joined' && p.account.uuid !== caller.uuid && p.account.uuid !== meUuid);

  // The sheet is modal: the app behind it leaves the tab order and the accessibility tree until it closes.
  useEffect(() => {
    previousFocus.current = document.activeElement;
    const app = document.querySelector('[data-ring-app]');
    app?.setAttribute('inert', '');
    return () => {
      app?.removeAttribute('inert');
      if (previousFocus.current instanceof HTMLElement) previousFocus.current.focus();
    };
  }, []);

  function failed(error: unknown, title: string) {
    if (error instanceof ApiError && error.code === 'invitation_not_ringing') toast({ type: 'info', title: 'Answered on another device' });
    else if (error instanceof ApiError && error.code === 'invitation_expired') toast({ type: 'info', title: `Missed call from ${label(caller)}` });
    else toast({ type: 'error', title, description: errorMessage(error) });
  }

  async function accept() {
    setBusy('accept');
    try {
      await store.accept(call.ringid, invitation.invitation_id);
      router.push(`/call?id=${encodeURIComponent(call.ringid)}`);
    } catch (error) {
      failed(error, 'Couldn’t answer');
      setBusy(null);
    }
  }

  async function decline(text?: string) {
    if (!declining && text === undefined) {
      setDeclining(true);
      return;
    }
    setBusy('decline');
    try {
      await store.decline(call.ringid, invitation.invitation_id, text?.trim() || undefined);
    } catch (error) {
      failed(error, 'Couldn’t decline');
      setBusy(null);
    }
  }

  async function silence() {
    setBusy('silence');
    try {
      await store.silence(call.ringid, invitation.invitation_id);
    } catch (error) {
      failed(error, 'Couldn’t silence the call');
      setBusy(null);
    }
  }

  const tooLong = codePoints(reason) > limit;

  return (
    <section className={styles.moment} data-ring-moment="" role="alertdialog" aria-modal="true" aria-labelledby="incoming-name" aria-describedby="incoming-status">
      <div className={styles.momentBody}>
        <span className={[styles.halo, styles.waves].join(' ')}>
          <PersonAvatar account={caller} size="xl" />
        </span>
        <div id="incoming-name">
          <NameBlock account={caller} moment />
        </div>
        <div id="incoming-status" className={styles.status}>
          <Pixel tone="moment">Is calling</Pixel>
          {others.length ? <Pixel tone="moment">With {others.map(p => label(p.account)).join(', ')}</Pixel> : null}
          {more > 0 ? <Pixel tone="moment">+{more} more calling</Pixel> : null}
        </div>

        {declining ? (
          <form
            className={styles.reasons}
            onSubmit={event => {
              event.preventDefault();
              if (reason.trim() && !tooLong) void decline(reason);
            }}
          >
            <Pixel tone="moment">Decline with a reason, or press decline again</Pixel>
            <div className={styles.reasonChips}>
              {REASONS.map(text => (
                <Button key={text} type="button" size="sm" variant="secondary" disabled={Boolean(busy)} onClick={() => void decline(text)}>
                  {text}
                </Button>
              ))}
            </div>
            <div className={styles.reasonField}>
              <Input
                label="Something else"
                value={reason}
                onChange={event => setReason(event.target.value)}
                error={tooLong ? `Keep it under ${limit} characters.` : undefined}
                autoComplete="off"
              />
              <Button type="submit" variant="secondary" aria-label="Decline with this reason" disabled={!reason.trim() || tooLong || Boolean(busy)}>
                <Send size={16} strokeWidth={1.75} aria-hidden="true" />
              </Button>
            </div>
          </form>
        ) : null}
      </div>

      <div className={styles.momentActions} ref={keepClearOfToasts}>
        <CallControl
          tone="danger"
          label={declining ? 'Decline, no reason' : 'Decline'}
          icon={<PhoneOff size={26} strokeWidth={1.75} aria-hidden="true" />}
          busy={busy === 'decline'}
          disabled={Boolean(busy) && busy !== 'decline'}
          onClick={() => void decline(declining ? '' : undefined)}
        />
        <CallControl
          tone="accept"
          label="Accept"
          icon={<Phone size={26} strokeWidth={1.75} aria-hidden="true" />}
          busy={busy === 'accept'}
          disabled={Boolean(busy) && busy !== 'accept'}
          autoFocus
          onClick={() => void accept()}
        />
      </div>
      <Button variant="ghost" className={styles.silence} disabled={Boolean(busy)} onClick={() => void silence()}>
        Silence
      </Button>
    </section>
  );
}
