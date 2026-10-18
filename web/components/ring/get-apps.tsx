'use client';

import { useSyncExternalStore } from 'react';
import { Download } from 'lucide-react';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import styles from './get-apps.module.css';

const RELEASES = 'https://github.com/teamofsilicons/silicon-ring/releases/latest';
const FILE = `${RELEASES}/download`;

type Platform = 'mac' | 'android' | 'windows' | 'linux';

/** The native apps, each linked to the latest release's file. Linux has several, so it opens the release. */
const APPS: { platform: Platform; label: string; href: string }[] = [
  { platform: 'mac', label: 'Mac', href: `${FILE}/Ring-macOS-1.0.0.dmg` },
  { platform: 'android', label: 'Android', href: `${FILE}/Ring-android-1.0.0.apk` },
  { platform: 'windows', label: 'Windows', href: `${FILE}/Ring-windows-x64.msi` },
  { platform: 'linux', label: 'Linux', href: RELEASES },
];

/** The visitor's platform, or null where there is no app to offer (iPhone, iPad) or it can't tell. */
function detect(): Platform | null {
  const ua = navigator.userAgent;
  if (/Android/i.test(ua)) return 'android';
  if (/iPhone|iPad|iPod/i.test(ua)) return null;
  // iPadOS asks for desktop sites with a Mac user agent; only the touch points give it away.
  if (/Macintosh/i.test(ua)) return navigator.maxTouchPoints > 1 ? null : 'mac';
  if (/Windows/i.test(ua)) return 'windows';
  if (/Linux|X11/i.test(ua)) return 'linux';
  return null;
}

const noSubscribe = () => () => {};

/**
 * Download buttons for the native apps, the visitor's own platform first. `highlight` also makes it
 * the primary button, for pages where downloading is the main thing to do.
 */
export function GetTheApps({ align = 'center', highlight = false }: { align?: 'center' | 'start'; highlight?: boolean }) {
  const mine = useSyncExternalStore(noSubscribe, detect, () => null);
  const apps = [...APPS].sort((a, b) => Number(b.platform === mine) - Number(a.platform === mine));
  return (
    <div className={[styles.apps, align === 'start' ? styles.start : ''].join(' ')}>
      <nav aria-label="Ring apps" className={styles.buttons}>
        {apps.map(app => (
          <ButtonLink key={app.platform} href={app.href} variant={highlight && app.platform === mine ? 'primary' : 'secondary'} size="sm">
            <Download size={14} strokeWidth={1.75} aria-hidden="true" />
            {app.label}
          </ButtonLink>
        ))}
      </nav>
      <a href={RELEASES} className={styles.all} target="_blank" rel="noreferrer">
        Every build, including Windows on Arm and Linux .deb and AppImage
      </a>
    </div>
  );
}
