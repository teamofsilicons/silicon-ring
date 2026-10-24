import Link from 'next/link';
import type { ReactNode } from 'react';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { Pixel } from './identity';
import { Wordmark } from './ring-mark';
import styles from './docs.module.css';

/** The links every public page and the sign-in screen carry. */
export const ABOUT_LINKS = [
  { href: '/privacy', label: 'Privacy' },
  { href: '/support', label: 'Support' },
  { href: '/docs', label: 'CLI docs' },
] as const;

/**
 * The frame of the pages anyone can read without signing in (privacy, support): the wordmark, Open Ring, the page in
 * a <main> landmark, and a footer with the other public pages.
 */
export function PublicPage({ children }: { children: ReactNode }) {
  return (
    // data-ring-app: the incoming-call sheet makes this inert while it is up.
    <div className={styles.page} data-ring-app="">
      <header className={styles.header}>
        <Link href="/" className={styles.brand} aria-label="Ring"><Wordmark /></Link>
        <ButtonLink href="/" as={Link} variant="secondary" size="sm">Open Ring</ButtonLink>
      </header>
      <main>{children}</main>
      <footer className={[styles.footer, styles.footerRow].join(' ')}>
        <Pixel>Ring sends no telemetry</Pixel>
        <nav aria-label="About Ring" className={styles.footerNav}>
          {ABOUT_LINKS.map(link => <Link key={link.href} href={link.href} className={styles.footerLink}>{link.label}</Link>)}
        </nav>
      </footer>
    </div>
  );
}
