import { Avatar } from '@teamofsilicons/silicon-ui/avatar';
import { Badge } from '@teamofsilicons/silicon-ui/badge';
import { label } from '@/lib/format';
import type { Account } from '@/lib/types';
import styles from './ring.module.css';

type AvatarSize = 'sm' | 'md' | 'lg' | 'xl';

export function PersonAvatar({ account, size = 'md', className }: { account: Account; size?: AvatarSize; className?: string }) {
  return <Avatar name={label(account)} src={account.pfp_url ?? undefined} size={size} className={className} />;
}

export function SiliconBadge({ account }: { account: Pick<Account, 'kind'> }) {
  if (account.kind !== 'silicon') return null;
  return <Badge tone="info" size="sm">SILICON</Badge>;
}

/** A small uppercase label in Geist Pixel Square: section headers, call status, timers, meta lines. */
export function Pixel({ children, tone = 'muted', className, as: Tag = 'span', keepCase = false }: {
  children: React.ReactNode;
  tone?: 'muted' | 'foreground' | 'danger' | 'success' | 'accent' | 'moment';
  className?: string;
  as?: 'span' | 'p' | 'h2' | 'div';
  /** Ids such as c:alice keep their case; labels are uppercase. */
  keepCase?: boolean;
}) {
  return <Tag className={[styles.pixel, styles[`tone-${tone}`], keepCase ? styles.keepCase : '', className].filter(Boolean).join(' ')}>{children}</Tag>;
}

/** The person's display name in the serif, or their id when they have none, with the id underneath. */
export function NameBlock({ account, size = 'call', moment = false }: { account: Account; size?: 'call' | 'title'; moment?: boolean }) {
  const name = account.display_name?.trim();
  return (
    <div className={[styles.nameBlock, moment ? styles.onMoment : ''].join(' ')}>
      <h1 className={size === 'call' ? styles.callName : styles.titleName}>{name || account.id}</h1>
      <div className={styles.nameMeta}>
        {name ? <Pixel tone={moment ? 'moment' : 'muted'} keepCase>{account.id}</Pixel> : null}
        <SiliconBadge account={account} />
      </div>
    </div>
  );
}
