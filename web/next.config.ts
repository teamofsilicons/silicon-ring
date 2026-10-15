import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { NextConfig } from 'next';
import { PHASE_DEVELOPMENT_SERVER } from 'next/constants';

const root = process.cwd();
const { version } = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')) as { version: string };

// Production is a static export in out/, served by Caddy with /v1 and /rtc proxied on the same origin.
// The development server proxies /v1 to a local ring-server instead (WebSockets connect to it directly).
export default function config(phase: string): NextConfig {
  const base: NextConfig = {
    turbopack: { root },
    poweredByHeader: false,
    reactStrictMode: true,
    agentRules: false,
    env: { NEXT_PUBLIC_RING_VERSION: version },
  };
  if (phase === PHASE_DEVELOPMENT_SERVER) {
    const api = process.env.RING_DEV_API || 'http://127.0.0.1:8765';
    // The socket goes to the same port on the page's own host name, so the host-only session cookie comes along.
    const wsPort = new URL(api).port || '80';
    return {
      ...base,
      env: { ...base.env, NEXT_PUBLIC_RING_DEV_WS_PORT: wsPort },
      async rewrites() { return [{ source: '/v1/:path*', destination: `${api}/v1/:path*` }]; },
    };
  }
  return { ...base, output: 'export', images: { unoptimized: true } };
}
