import type { MetadataRoute } from 'next';

export const dynamic = 'force-static';

export default function manifest(): MetadataRoute.Manifest {
  return {
    name: 'Silicon Ring',
    short_name: 'Ring',
    description: 'Voice calls between carbons and silicons.',
    start_url: '/',
    scope: '/',
    display: 'standalone',
    background_color: '#181818',
    theme_color: '#181818',
    icons: [
      { src: '/icons/icon-192.png', sizes: '192x192', type: 'image/png' },
      { src: '/icons/icon-512.png', sizes: '512x512', type: 'image/png' },
      { src: '/icon.svg', sizes: 'any', type: 'image/svg+xml' },
    ],
  };
}
