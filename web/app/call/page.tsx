import type { Metadata } from 'next';
import { Suspense } from 'react';
import { CallScreen } from '@/components/ring/call-screen';

export const metadata: Metadata = { title: 'Call' };

// One static page for every call: the call is chosen by ?id=, since a static export has no dynamic routes.
export default function Page() {
  return <Suspense fallback={null}><CallScreen /></Suspense>;
}
