'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { useEffect, type ReactNode } from 'react';
import { SiliconProvider, type SiliconLinkProps } from '@teamofsilicons/silicon-ui/provider';
import { ToastStackProvider } from '@teamofsilicons/silicon-ui/toast-stack';
import { applyAppearance, readAppearance } from '@/lib/theme';
import { DialerProvider } from './dialer';
import { IncomingCallLayer } from './incoming-call';
import { RingProvider, useRing } from './ring-context';
import { Shell } from './shell';
import { SignIn, SiliconNotice, Splash, Unreachable } from './sign-in';
import { Toasts } from './toasts';

function SiliconNextLink(props: SiliconLinkProps) {
  return <Link {...props} />;
}

/** Follows the system appearance while the person has chosen "System". */
function useSystemAppearance() {
  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    const update = () => {
      if (readAppearance() === 'system') applyAppearance('system');
    };
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
}

/** Pages anyone can read without signing in. */
const PUBLIC_PAGES = ['/docs', '/privacy', '/support'];

function AppFrame({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const status = useRing(state => state.status);
  const kind = useRing(state => state.me?.account.kind);
  useSystemAppearance();

  // The public pages need no sign-in, but a signed-in carbon reading them still gets calls (the ringtone plays on
  // every page).
  if (PUBLIC_PAGES.some(page => pathname === page || pathname.startsWith(`${page}/`))) {
    return (
      <>
        {children}
        {status === 'signed-in' && kind === 'carbon' ? <IncomingCallLayer /> : null}
      </>
    );
  }
  if (status === 'loading') return <Splash />;
  if (status === 'unreachable') return <Unreachable />;
  if (status === 'signed-out') return <SignIn />;
  if (kind === 'silicon') return <SiliconNotice />;
  return (
    <DialerProvider>
      <Shell>{children}</Shell>
      <IncomingCallLayer />
    </DialerProvider>
  );
}

export function Providers({ children }: { children: ReactNode }) {
  return (
    <SiliconProvider link={SiliconNextLink}>
      <ToastStackProvider>
        <RingProvider>
          <AppFrame>{children}</AppFrame>
        </RingProvider>
        <Toasts />
      </ToastStackProvider>
    </SiliconProvider>
  );
}
