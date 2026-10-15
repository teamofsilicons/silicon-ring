import Link from 'next/link';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { EmptyState } from '@teamofsilicons/silicon-ui/empty-state';
import { RingRings } from '@/components/ring/ring-mark';

export default function NotFound() {
  return (
    <EmptyState
      icon={<RingRings size={28} />}
      title="Nothing rings here"
      description="This page doesn’t exist."
      action={<ButtonLink href="/" as={Link} variant="secondary">Back to Ring</ButtonLink>}
    />
  );
}
