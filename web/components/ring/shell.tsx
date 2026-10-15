'use client';

import Link from 'next/link';
import { usePathname, useRouter } from 'next/navigation';
import { useState, type CSSProperties, type ReactNode } from 'react';
import { History, PhoneCall, Settings, Voicemail } from 'lucide-react';
import { Badge } from '@teamofsilicons/silicon-ui/badge';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { errorMessage } from '@/lib/api';
import { deviceLabel, duration, label, myParticipant, otherPeople } from '@/lib/format';
import { useDialer } from './dialer';
import { useMe, useMediaCall, useNow } from './hooks';
import { Pixel } from './identity';
import { useRing, useRingStore } from './ring-context';
import { Wordmark } from './ring-mark';
import styles from './shell.module.css';

const NAV = [
  { href: '/', label: 'Recents', icon: History },
  { href: '/voicemail', label: 'Voicemail', icon: Voicemail },
  { href: '/settings', label: 'Settings', icon: Settings },
] as const;

function activeIndex(pathname: string) {
  if (pathname.startsWith('/voicemail')) return 1;
  if (pathname.startsWith('/settings')) return 2;
  if (pathname === '/' || pathname.startsWith('/history')) return 0;
  return -1;
}

export function Shell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const openDialer = useDialer();
  const unread = useRing(state => state.unreadVoicemail);
  const connection = useRing(state => state.connection);
  const index = activeIndex(pathname);
  const onCallScreen = pathname.startsWith('/call');
  const pill = { '--index': Math.max(index, 0) } as CSSProperties;

  const links = NAV.map((item, i) => {
    const Icon = item.icon;
    return (
      <Link key={item.href} href={item.href} className={styles.navLink} aria-current={i === index ? 'page' : undefined}>
        <Icon size={18} strokeWidth={1.6} aria-hidden="true" />
        <span className={styles.navLabel}>{item.label}</span>
        {item.href === '/voicemail' && unread > 0 ? <span className={styles.count}><Badge size="sm" tone="info" aria-label={`${unread} unread`}>{unread > 99 ? '99+' : unread}</Badge></span> : null}
      </Link>
    );
  });

  return (
    <div className={styles.shell} data-ring-app="" data-call-screen={onCallScreen || undefined}>
      <aside className={styles.sidebar} aria-label="Ring">
        <Link href="/" className={styles.brand} aria-label="Ring home"><Wordmark /></Link>
        <nav className={styles.nav} aria-label="Main">
          {index >= 0 ? <span className={styles.pill} style={pill} aria-hidden="true" /> : null}
          {links}
        </nav>
        <div className={styles.sidebarFooter}>
          {connection === 'reconnecting' ? <Pixel tone="danger">Reconnecting…</Pixel> : null}
        </div>
      </aside>

      <div className={styles.main}>
        <header className={styles.topbar}>
          <Link href="/" className={styles.brand} aria-label="Ring home"><Wordmark /></Link>
          {connection === 'reconnecting' ? <Pixel tone="danger">Reconnecting…</Pixel> : null}
        </header>
        {onCallScreen ? null : <CallBanner />}
        <main className={styles.content}>{children}</main>
      </div>

      {onCallScreen ? null : (
        <>
          <div className={styles.fab}>
            <Button variant="primary" size="lg" onClick={() => openDialer()}>
              <PhoneCall size={18} strokeWidth={1.75} aria-hidden="true" />
              New call
            </Button>
          </div>
          <nav className={styles.tabbar} aria-label="Main">
            {index >= 0 ? <span className={styles.tabPill} style={pill} aria-hidden="true" /> : null}
            {links}
          </nav>
        </>
      )}
    </div>
  );
}

/** Shown on every screen except the call itself while a call is live: return to it, or bring it to this browser. */
function CallBanner() {
  const store = useRingStore();
  const me = useMe();
  const calls = useRing(state => state.calls);
  const { media, call: mediaCall } = useMediaCall();
  const now = useNow();
  const { toast } = useToastStack();
  const router = useRouter();
  const [switching, setSwitching] = useState(false);
  const meUuid = me?.account.uuid;

  if (mediaCall && (mediaCall.state === 'active' || mediaCall.state === 'ringing' || mediaCall.state === 'voicemail')) {
    const names = otherPeople(mediaCall, meUuid).map(label).join(', ');
    const timer = mediaCall.answered_at ? duration(now - Date.parse(mediaCall.answered_at)) : mediaCall.state === 'ringing' ? 'RINGING' : 'VOICEMAIL';
    return (
      <div className={styles.banner} role="status">
        <span className={styles.bannerText}>On a call with {names} <Pixel>{timer}</Pixel></span>
        <ButtonLink href={`/call?id=${encodeURIComponent(mediaCall.ringid)}`} size="sm" variant="secondary" as={Link}>Return to call</ButtonLink>
      </div>
    );
  }
  if (media) return null;

  const elsewhere = Object.values(calls).find(call => {
    if (call.state !== 'active') return false;
    const mine = myParticipant(call, meUuid);
    return mine?.state === 'joined' && mine.device && mine.device.device_id !== me?.device.device_id;
  });
  if (!elsewhere) return null;
  const device = myParticipant(elsewhere, meUuid)?.device;
  const names = otherPeople(elsewhere, meUuid).map(label).join(', ');
  return (
    <div className={styles.banner} role="status">
      <span className={styles.bannerText}>On a call with {names} on {deviceLabel(device)}</span>
      <Button
        size="sm"
        variant="secondary"
        loading={switching}
        onClick={async () => {
          setSwitching(true);
          try {
            await store.switchHere(elsewhere.ringid);
            router.push(`/call?id=${encodeURIComponent(elsewhere.ringid)}`);
          } catch (error) {
            toast({ type: 'error', title: 'Couldn’t switch the call', description: errorMessage(error) });
          } finally {
            setSwitching(false);
          }
        }}
      >
        Switch to this browser
      </Button>
    </div>
  );
}
