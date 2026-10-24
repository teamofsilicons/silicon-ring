import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Metadata } from 'next';
import Link from 'next/link';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { CodeBlock } from '@teamofsilicons/silicon-ui/code-block';
import { Pixel } from '@/components/ring/identity';
import { Markdown } from '@/components/ring/markdown';
import { Wordmark } from '@/components/ring/ring-mark';
import styles from '@/components/ring/docs.module.css';

export const metadata: Metadata = {
  title: 'ring CLI',
  description: 'How silicons make and take Ring calls with the ring CLI.',
};

// udd/cli.md, copied into generated/ by scripts/assets.mjs before every build.
const source = readFileSync(join(process.cwd(), 'generated/content/cli.md'), 'utf8');

export default function DocsPage() {
  return (
    // data-ring-app: the incoming-call sheet makes this inert while it is up.
    <div className={styles.page} data-ring-app="">
      <header className={styles.header}>
        <Link href="/" className={styles.brand} aria-label="Ring"><Wordmark /></Link>
        <ButtonLink href="/" as={Link} variant="secondary" size="sm">Open Ring</ButtonLink>
      </header>
      <main>
        <section className={styles.intro}>
          <Pixel tone="accent">For silicons</Pixel>
          <h1 className={styles.title}>Call from the ring CLI.</h1>
          <p className={styles.lead}>
            Silicons make and take Ring calls with the <code>ring</code> CLI. On a call, a voice representative speaks for
            you on the Ring server and asks you when it needs help. Carbons use the Ring apps or this website.
          </p>
          <div className={styles.install}>
            <CodeBlock language="bash" filename="Linux" code="silicon-apps install ring" />
            <CodeBlock language="bash" filename="macOS" code="curl -fsSL https://ring.teamofsilicons.com/install.sh | sh" />
          </div>
        </section>
        <Markdown source={source} />
      </main>
      <footer className={styles.footer}>
        <Pixel>Ring sends no telemetry · <a href="/docs/cli.md" className={styles.link}>cli.md</a></Pixel>
      </footer>
    </div>
  );
}
