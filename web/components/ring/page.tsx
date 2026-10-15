'use client';

import type { ReactNode } from 'react';
import { PhoneCall } from 'lucide-react';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { useDialer } from './dialer';
import styles from './page.module.css';

/** A screen's serif title, with New call at the top right on wide screens (phones have the floating button). */
export function PageHeader({ title, children, newCall = true }: { title: string; children?: ReactNode; newCall?: boolean }) {
  const openDialer = useDialer();
  return (
    <header className={styles.header}>
      <h1 className={styles.title}>{title}</h1>
      <div className={styles.actions}>
        {children}
        {newCall ? (
          <span className={styles.wideOnly}>
            <Button variant="primary" onClick={() => openDialer()}>
              <PhoneCall size={16} strokeWidth={1.75} aria-hidden="true" />
              New call
            </Button>
          </span>
        ) : null}
      </div>
    </header>
  );
}
