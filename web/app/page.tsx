import type { Metadata } from 'next';
import { Recents } from '@/components/ring/recents';

export const metadata: Metadata = { title: 'Recents' };

export default function Page() {
  return <Recents />;
}
