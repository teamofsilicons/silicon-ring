import type { Metadata } from 'next';
import { Suspense } from 'react';
import { CallDetailsScreen } from '@/components/ring/call-details';

export const metadata: Metadata = { title: 'Call details' };

export default function Page() {
  return <Suspense fallback={null}><CallDetailsScreen /></Suspense>;
}
