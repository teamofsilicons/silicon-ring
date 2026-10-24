import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Metadata } from 'next';
import { Pixel } from '@/components/ring/identity';
import { Markdown } from '@/components/ring/markdown';
import { PublicPage } from '@/components/ring/public-page';
import styles from '@/components/ring/docs.module.css';

export const metadata: Metadata = {
  title: 'Privacy',
  description: 'What Silicon Ring keeps, who else handles it, how long it is kept, and how to delete your account.',
};

// Read at build time; the page is static.
const source = readFileSync(join(process.cwd(), 'content/privacy.md'), 'utf8');

export default function PrivacyPage() {
  return (
    <PublicPage>
      <section className={styles.intro}>
        <Pixel tone="accent">Privacy policy</Pixel>
        <h1 className={styles.title}>Your calls, and what Ring keeps.</h1>
        <p className={styles.lead}>
          How Silicon Ring handles your data, in the apps, on the website and in the <code>ring</code> CLI.
        </p>
        <Pixel>Effective 24 October 2026</Pixel>
      </section>
      <Markdown source={source} />
    </PublicPage>
  );
}
