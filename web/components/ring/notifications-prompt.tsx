'use client';

import { useState } from 'react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { errorMessage } from '@/lib/api';
import { pushSupported } from '@/lib/push';
import { useRing, useRingStore } from './ring-context';
import styles from './page.module.css';

const DISMISSED = 'ring.notifications-prompt';

function wasDismissed() {
  try {
    return localStorage.getItem(DISMISSED) === '1';
  } catch {
    return false;
  }
}

/** Offers Web Push once, so calls still ring when the Ring tab is closed. */
export function NotificationsPrompt() {
  const store = useRingStore();
  const push = useRing(state => state.push);
  const ringEnabled = useRing(state => state.me?.device.ring_enabled ?? false);
  const { toast } = useToastStack();
  const [hidden, setHidden] = useState(() => wasDismissed() || !pushSupported());
  const [busy, setBusy] = useState(false);

  if (hidden || push !== 'default' || !ringEnabled) return null;

  function dismiss() {
    try {
      localStorage.setItem(DISMISSED, '1');
    } catch {
      // Storage can be unavailable; hiding it for this visit is enough.
    }
    setHidden(true);
  }

  return (
    <Alert tone="info" title="Ring this browser when Ring is closed" onDismiss={dismiss} className={styles.prompt}>
      <span className={styles.promptBody}>
        Allow notifications and incoming calls reach you even when this tab isn’t open.
        <Button
          size="sm"
          variant="primary"
          loading={busy}
          onClick={async () => {
            setBusy(true);
            try {
              const result = await store.enableNotifications();
              if (result === 'granted') toast({ type: 'success', title: 'Notifications are on' });
            } catch (error) {
              toast({ type: 'error', title: 'Notifications didn’t turn on', description: errorMessage(error) });
            } finally {
              setBusy(false);
            }
          }}
        >
          Turn on notifications
        </Button>
      </span>
    </Alert>
  );
}
