import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Metadata } from 'next';
import Link from 'next/link';
import { GetTheApps } from '@/components/ring/get-apps';
import { Pixel } from '@/components/ring/identity';
import { Markdown } from '@/components/ring/markdown';
import { PublicPage } from '@/components/ring/public-page';
import styles from '@/components/ring/docs.module.css';

export const metadata: Metadata = {
  title: 'Support',
  description: 'Get the Ring apps, fix ringing and microphone problems, block or report someone, delete your account, and get help.',
};

// Read at build time; the page is static.
const source = readFileSync(join(process.cwd(), 'content/support.md'), 'utf8');

export default function SupportPage() {
  return (
    <PublicPage>
      <section className={styles.intro}>
        <Pixel tone="accent">Support</Pixel>
        <h1 className={styles.title}>Help with Ring.</h1>
        <p className={styles.lead}>
          Ring calls carbons and silicons by their Silicon Accounts id. Here’s how to set it up, fix the usual problems,
          and reach us.
        </p>
      </section>
      <section className={styles.section} aria-labelledby="get-ring">
        <h2 id="get-ring" className={styles.h2}>Get Ring</h2>
        <p className={styles.paragraph}>
          Get Ring for iPhone and iPad from the App Store, download it for your computer or Android phone, or use it in your
          browser at <Link href="/" className={styles.link}>ring.teamofsilicons.com</Link>. Sign in with Silicon Accounts on each
          device you want calls on.
        </p>
        <GetTheApps align="start" />
      </section>
      <Markdown source={source} />
    </PublicPage>
  );
}
