// Copies the shared Ring assets (fonts, sounds, the Ring mark) and the CLI docs into the web app before it builds.
// The originals live in ../assets and ../udd and are never edited from here; the copies are gitignored.
import { copyFileSync, existsSync, mkdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = fileURLToPath(new URL('..', import.meta.url));
const repo = join(web, '..');

const files = [
  ['assets/fonts/bdo-grotesk/BDOGrotesk-Regular.woff2', 'generated/fonts/BDOGrotesk-Regular.woff2'],
  ['assets/fonts/bdo-grotesk/BDOGrotesk-Medium.woff2', 'generated/fonts/BDOGrotesk-Medium.woff2'],
  ['assets/fonts/bdo-grotesk/BDOGrotesk-Bold.woff2', 'generated/fonts/BDOGrotesk-Bold.woff2'],
  ['assets/fonts/hedvig-letters-serif/HedvigLettersSerif-Regular.woff2', 'generated/fonts/HedvigLettersSerif-Regular.woff2'],
  ['assets/fonts/geist-pixel/GeistPixel-Square.woff2', 'generated/fonts/GeistPixel-Square.woff2'],
  ['udd/cli.md', 'generated/content/cli.md'],
  ['udd/cli.md', 'public/docs/cli.md'],
  ['assets/sounds/ringback.wav', 'public/sounds/ringback.wav'],
  ['assets/sounds/ringtone.wav', 'public/sounds/ringtone.wav'],
  ['assets/sounds/connected.wav', 'public/sounds/connected.wav'],
  ['assets/sounds/ended.wav', 'public/sounds/ended.wav'],
  ['assets/icon/icon.svg', 'public/icon.svg'],
  ['assets/icon/ring-logo.svg', 'public/icons/ring-logo.svg'],
  ['assets/icon/png/icon-32.png', 'public/icons/icon-32.png'],
  ['assets/icon/png/icon-180.png', 'public/icons/icon-180.png'],
  ['assets/icon/png/icon-192.png', 'public/icons/icon-192.png'],
  ['assets/icon/png/icon-512.png', 'public/icons/icon-512.png'],
  ['assets/icon/png/monochrome-96.png', 'public/icons/badge-96.png'],
];

let copied = 0;
for (const [from, to] of files) {
  const source = join(repo, from);
  const target = join(web, to);
  if (!existsSync(source)) throw new Error(`Missing shared asset ${from}.`);
  if (existsSync(target) && statSync(target).mtimeMs >= statSync(source).mtimeMs && statSync(target).size === statSync(source).size) continue;
  mkdirSync(dirname(target), { recursive: true });
  copyFileSync(source, target);
  copied++;
}
if (copied) console.log(`Copied ${copied} shared asset${copied === 1 ? '' : 's'} into web/.`);
