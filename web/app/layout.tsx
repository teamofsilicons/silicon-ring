import type { Metadata, Viewport } from 'next';
import type { ReactNode } from 'react';
import '@teamofsilicons/silicon-ui/styles.css';
import './tokens.css';
import './globals.css';
import { Providers } from '@/components/ring/providers';
import { themeBootScript } from '@/lib/theme';
import { bdoGrotesk, geistPixelSquare, hedvigLettersSerif } from './fonts';

export const metadata: Metadata = {
  title: { default: 'Ring', template: '%s · Ring' },
  description: 'Silicon Ring: voice calls between carbons and silicons.',
  applicationName: 'Ring',
  icons: {
    icon: [{ url: '/icon.svg', type: 'image/svg+xml' }, { url: '/icons/icon-32.png', sizes: '32x32', type: 'image/png' }],
    apple: [{ url: '/icons/icon-180.png', sizes: '180x180' }],
  },
  appleWebApp: { capable: true, title: 'Ring', statusBarStyle: 'black-translucent' },
  formatDetection: { telephone: false },
};

export const viewport: Viewport = {
  width: 'device-width',
  initialScale: 1,
  viewportFit: 'cover',
  themeColor: [
    { media: '(prefers-color-scheme: light)', color: '#fffdf9' },
    { media: '(prefers-color-scheme: dark)', color: '#181818' },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html
      lang="en"
      data-theme="light"
      data-corners="round"
      className={[bdoGrotesk.variable, hedvigLettersSerif.variable, geistPixelSquare.variable].join(' ')}
      suppressHydrationWarning
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
      </head>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
