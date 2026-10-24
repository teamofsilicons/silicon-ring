'use client';

import Link from 'next/link';
import { useEffect, useState } from 'react';
import { ArrowRight, BookOpen } from 'lucide-react';
import { Alert, type AlertTone } from '@teamofsilicons/silicon-ui/alert';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { CodeBlock } from '@teamofsilicons/silicon-ui/code-block';
import type { Notice } from '@/lib/store';
import { useDocumentTitle } from './hooks';
import { Pixel } from './identity';
import { useRing, useRingStore } from './ring-context';
import { RingMark, Wordmark } from './ring-mark';
import { GetTheApps } from './get-apps';
import { ABOUT_LINKS } from './public-page';
import styles from './sign-in.module.css';

const SIGN_IN_ERRORS: Record<string, string> = {
  access_denied: 'Sign-in was cancelled.',
  state_mismatch: 'That sign-in link expired. Try again.',
  login_expired: 'That sign-in link expired. Try again.',
  signin_failed: 'Silicon Accounts didn’t complete the sign-in. Try again.',
  dependency_unavailable: 'Silicon Accounts could not be reached. Try again in a moment.',
};

const NOTICE_TONES: Record<Notice['type'], AlertTone> = { success: 'success', info: 'info', warning: 'warning', error: 'danger' };

function readSignInError() {
  if (typeof window === 'undefined') return null;
  return new URL(window.location.href).searchParams.get('signin_error');
}

/** Where to come back to after signing in: the current page without the error parameter. */
function returnPath() {
  if (typeof window === 'undefined') return '/';
  const params = new URLSearchParams(window.location.search);
  params.delete('signin_error');
  const query = params.toString();
  return `${window.location.pathname}${query ? `?${query}` : ''}`;
}

function useSignInError() {
  const [error] = useState(readSignInError);
  useEffect(() => {
    const url = new URL(window.location.href);
    if (!url.searchParams.has('signin_error')) return;
    url.searchParams.delete('signin_error');
    window.history.replaceState(window.history.state, '', `${url.pathname}${url.search}${url.hash}`);
  }, []);
  return error;
}

export function SignIn() {
  const error = useSignInError();
  const meta = useRing(state => state.meta);
  // Why this browser was signed out, such as a deleted account.
  const notice = useRing(state => state.signedOutNotice);
  const [next] = useState(returnPath);
  useDocumentTitle(error === 'silicon_use_cli' ? 'Ring for silicons' : 'Sign in · Ring');

  if (error === 'silicon_use_cli') return <SiliconNotice />;

  return (
    <main className={styles.page}>
      <section className={styles.card}>
        <RingMark size={88} />
        <Wordmark className={styles.wordmark} mark={false} />
        <h1 className={styles.headline}>Calls between carbons and silicons.</h1>
        {error ? (
          <Alert tone="danger" title="You’re not signed in" className={styles.alert}>
            {SIGN_IN_ERRORS[error] ?? `Sign-in failed (${error}). Try again.`}
          </Alert>
        ) : notice ? (
          <Alert tone={NOTICE_TONES[notice.type]} title={notice.title} className={styles.alert}>{notice.description}</Alert>
        ) : null}
        <ButtonLink href={`/v1/auth/web/start?next=${encodeURIComponent(next)}`} variant="primary" size="lg" className={styles.cta}>
          Continue with Silicon Accounts
          <ArrowRight size={16} strokeWidth={1.75} aria-hidden="true" />
        </ButtonLink>
        <p className={styles.small}>
          New to Team of Silicons? Silicon Accounts creates your account as you sign in.
          {meta ? <> <a href={meta.accounts_url} className={styles.link} target="_blank" rel="noreferrer">accounts.teamofsilicons.com</a></> : null}
        </p>
      </section>
      <section className={styles.apps} aria-label="Get the apps">
        <Pixel>Ring on your devices</Pixel>
        <GetTheApps />
      </section>
      <footer className={styles.footer}>
        <Pixel>Silicons use the ring CLI · Ring sends no telemetry</Pixel>
        <nav aria-label="About Ring" className={styles.footerNav}>
          {ABOUT_LINKS.map(link => <Link key={link.href} href={link.href} className={styles.footerLink}>{link.label}</Link>)}
        </nav>
      </footer>
    </main>
  );
}

const START = `silicon-accounts login --app ring -q | ring login --slt-stdin
ring call init c:alice --start "Hi Alice, it's TOS."`;

export function SiliconNotice() {
  const store = useRingStore();
  const me = useRing(state => state.me);
  const [origin] = useState(() => (typeof window === 'undefined' ? 'https://ring.teamofsilicons.com' : window.location.origin));
  useDocumentTitle('Ring for silicons');

  return (
    <main className={styles.page}>
      <section className={[styles.card, styles.wide].join(' ')}>
        <RingMark size={64} />
        <Pixel tone="accent">For silicons</Pixel>
        <h1 className={styles.headline}>Silicons call from the ring CLI.</h1>
        <p className={styles.lead}>
          On a call, a voice representative speaks for you on the Ring server and asks you for help when it needs it.
          The web app is for carbons, so install the CLI and sign in there.
        </p>
        <div className={styles.code}>
          <CodeBlock language="bash" filename="Linux" code="silicon-apps install ring" />
          <CodeBlock language="bash" filename="macOS" code={`curl -fsSL ${origin}/install.sh | sh`} />
          <CodeBlock language="bash" filename="Sign in, then call" code={START} />
        </div>
        <div className={styles.actions}>
          <ButtonLink href="/docs" variant="primary" as={Link}>
            <BookOpen size={16} strokeWidth={1.75} aria-hidden="true" />
            Read the CLI docs
          </ButtonLink>
          {me ? <Button variant="secondary" onClick={() => void store.signOut()}>Sign out</Button> : null}
        </div>
      </section>
      <footer className={styles.footer}>
        <Pixel>Ring sends no telemetry</Pixel>
        <nav aria-label="About Ring" className={styles.footerNav}>
          {ABOUT_LINKS.map(link => <Link key={link.href} href={link.href} className={styles.footerLink}>{link.label}</Link>)}
        </nav>
      </footer>
    </main>
  );
}

export function Splash() {
  useDocumentTitle('Ring');
  return (
    <main className={styles.splash} aria-busy="true" aria-label="Loading Ring">
      <RingMark size={56} className={styles.pulse} />
    </main>
  );
}

export function Unreachable() {
  const store = useRingStore();
  useDocumentTitle('Ring');
  // Comes back by itself once the network or the server does, as after a laptop wakes up.
  useEffect(() => {
    const retry = () => void store.load(true);
    const timer = setInterval(retry, 15_000);
    window.addEventListener('online', retry);
    return () => {
      clearInterval(timer);
      window.removeEventListener('online', retry);
    };
  }, [store]);
  return (
    <main className={styles.page}>
      <section className={styles.card}>
        <RingMark size={64} />
        <h1 className={styles.headline}>Ring can’t be reached.</h1>
        <p className={styles.lead}>Check your connection, then try again.</p>
        <Button variant="primary" onClick={() => void store.load()}>Try again</Button>
      </section>
    </main>
  );
}
