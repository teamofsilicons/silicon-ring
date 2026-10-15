import localFont from 'next/font/local';

// The teamofsilicons.com typefaces, self-hosted from ../assets/fonts (copied by scripts/assets.mjs). All are OFL 1.1.
export const bdoGrotesk = localFont({
  src: [
    { path: '../generated/fonts/BDOGrotesk-Regular.woff2', weight: '400', style: 'normal' },
    { path: '../generated/fonts/BDOGrotesk-Medium.woff2', weight: '500', style: 'normal' },
    { path: '../generated/fonts/BDOGrotesk-Bold.woff2', weight: '700', style: 'normal' },
  ],
  variable: '--font-bdo-grotesk',
  display: 'swap',
  fallback: ['system-ui', 'sans-serif'],
});

export const hedvigLettersSerif = localFont({
  src: '../generated/fonts/HedvigLettersSerif-Regular.woff2',
  weight: '400',
  variable: '--font-hedvig-letters-serif',
  display: 'swap',
  fallback: ['Georgia', 'serif'],
});

export const geistPixelSquare = localFont({
  src: '../generated/fonts/GeistPixel-Square.woff2',
  weight: '400',
  variable: '--font-geist-pixel-square',
  display: 'swap',
  fallback: ['ui-monospace', 'monospace'],
});
