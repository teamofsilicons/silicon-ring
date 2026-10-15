import type { Metadata } from 'next';
import { SettingsScreen } from '@/components/ring/settings';

export const metadata: Metadata = { title: 'Settings' };

export default function Page() {
  return <SettingsScreen />;
}
