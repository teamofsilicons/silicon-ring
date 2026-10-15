'use client';

/*
  LOCAL EXCEPTION to Silicon UI (listed in web/README.md).
  Replaces: a round, filled call-control button (accept in success, decline and hang up in danger, mute toggling to
  filled ink) with its label underneath. Silicon UI's Button has no circular shape and its danger variant is outlined.
  Goes away when Silicon UI ships a call-control (round icon button with success/danger fills); then switch these
  call sites to it and delete this folder.
*/

import type { ReactNode } from 'react';
import styles from './call-control.module.css';

export type CallControlTone = 'accept' | 'danger' | 'neutral' | 'moment';

export interface CallControlProps {
  label: string;
  icon: ReactNode;
  tone?: CallControlTone;
  size?: 'md' | 'lg';
  /** For toggles such as mute: the button reads as pressed and fills with ink. */
  pressed?: boolean;
  busy?: boolean;
  disabled?: boolean;
  hideLabel?: boolean;
  autoFocus?: boolean;
  onClick: () => void;
}

export function CallControl({ label, icon, tone = 'neutral', size = 'lg', pressed, busy, disabled, hideLabel, autoFocus, onClick }: CallControlProps) {
  return (
    <span className={styles.control}>
      <button
        type="button"
        className={[styles.button, styles[tone], styles[size]].join(' ')}
        aria-label={label}
        aria-pressed={pressed}
        aria-busy={busy || undefined}
        data-pressed={pressed || undefined}
        disabled={disabled || busy}
        autoFocus={autoFocus}
        onClick={onClick}
      >
        {icon}
      </button>
      {hideLabel ? null : <span className={styles.label} aria-hidden="true">{label}</span>}
    </span>
  );
}
