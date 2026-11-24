'use client';

import { useEffect, useRef, useState } from 'react';
import { ArrowUpRight, AudioWaveform, MessageSquareText } from 'lucide-react';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { Dialog, DialogContent } from '@teamofsilicons/silicon-ui/dialog';
import type { RecordingNoticeAction, RecordingNoticeRequest } from '@/lib/recording-notice';
import { Pixel } from './identity';
import { useRing, useRingStore } from './ring-context';
import styles from './recording-notice.module.css';

const TITLE = 'Ring records calls';
const INDICATOR = 'Recorded · Transcribed';

/** What "Not now" does, for each thing that can wait for the notice. */
function notNowHint(action: RecordingNoticeAction) {
  switch (action.kind) {
    case 'welcome': return 'Asks again before your first call';
    case 'place': return 'Doesn’t place the call';
    case 'answer': return 'Doesn’t answer the call';
    case 'switch': return 'Leaves the call on your other device';
    case 'rejoin': return 'Doesn’t rejoin the call';
  }
}

/** A small red dot and "Recorded · Transcribed", on the outgoing and in-call screens and atop the notice. */
export function RecordingIndicator({ onCall = true }: { onCall?: boolean }) {
  return (
    <span className={styles.indicator} title={onCall ? 'This call is recorded and transcribed' : undefined}>
      <span className={styles.dot} aria-hidden="true" />
      <Pixel>{INDICATOR}</Pixel>
    </span>
  );
}

/**
 * The recording notice (as in the native apps, `apple/RingKit/Views/RecordingNoticeView.swift`): Ring records and
 * transcribes calls, and which providers process the audio. It shows once after sign-in (`offer`), and before placing,
 * answering, switching or rejoining a call until this browser agrees. "Agree and continue" carries on with what waited
 * for it; "Not now", Escape or the close button drop it. A press outside the card does nothing.
 */
export function RecordingNotice({ offer = false }: { offer?: boolean }) {
  const store = useRingStore();
  const request = useRing(state => state.recordingNotice);
  // The copy stays while the dialog animates out.
  const [shown, setShown] = useState<RecordingNoticeRequest | null>(request);
  if (request && request !== shown) setShown(request);
  const agree = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (offer) store.offerRecordingNotice();
  }, [offer, store]);

  const current = request ?? shown;
  return (
    <Dialog
      open={Boolean(request)}
      onOpenChange={open => {
        if (!open && request) store.dismissRecordingNotice(request.id);
      }}
    >
      <DialogContent
        title={TITLE}
        onPointerDownOutside={event => event.preventDefault()}
        // Enter agrees, as in the desktop apps: Agree and continue has the focus.
        onOpenAutoFocus={event => {
          event.preventDefault();
          agree.current?.focus();
        }}
      >
        <div className={styles.body}>
          <RecordingIndicator onCall={false} />
          <p className={styles.lead}>
            Every call is recorded and transcribed, so the people on it can read it and play it back later. Everyone on a
            call sees the same transcript.
          </p>
          <ul className={styles.items}>
            <li className={styles.item}>
              <MessageSquareText size={16} strokeWidth={1.5} aria-hidden="true" className={styles.icon} />
              <span>Deepgram transcribes what people say.</span>
            </li>
            <li className={styles.item}>
              <AudioWaveform size={16} strokeWidth={1.5} aria-hidden="true" className={styles.icon} />
              <span>When a silicon is on the call, OpenAI produces its voice and hears the call.</span>
            </li>
          </ul>
          <p className={styles.tip}>Some places require everyone’s consent to record a call. Let the people you call know.</p>
          <ButtonLink href="/privacy" target="_blank" rel="noopener" variant="ghost" size="sm" className={styles.privacy}>
            Privacy Policy
            <ArrowUpRight size={14} strokeWidth={1.5} aria-hidden="true" />
          </ButtonLink>
          <div className={styles.buttons}>
            <Button
              variant="ghost"
              title={current ? notNowHint(current.action) : undefined}
              onClick={() => {
                if (request) store.dismissRecordingNotice(request.id);
              }}
            >
              Not now
            </Button>
            <Button
              ref={agree}
              variant="primary"
              onClick={() => {
                if (request) store.agreeToRecordingNotice(request.id);
              }}
            >
              Agree and continue
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
