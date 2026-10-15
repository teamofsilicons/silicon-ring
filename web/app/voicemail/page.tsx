import type { Metadata } from 'next';
import { Suspense } from 'react';
import { VoicemailScreen } from '@/components/ring/voicemail';

export const metadata: Metadata = { title: 'Voicemail' };

export default function Page() {
  return <Suspense fallback={null}><VoicemailScreen /></Suspense>;
}
