import styles from './ring.module.css';

/** The Ring mark (assets/icon/icon.svg): concentric cream rings around a cream dot on ink. */
export function RingMark({ size = 88, className }: { size?: number; className?: string }) {
  return (
    <svg className={[styles.mark, className].filter(Boolean).join(' ')} width={size} height={size} viewBox="0 0 200 200" fill="none" aria-hidden="true">
      <rect width="200" height="200" rx="44" className={styles.markInk} />
      <g className={styles.markLines}>
        <circle cx="99.5" cy="99.5" r="11.5" className={styles.markDot} />
        <circle cx="99.5" cy="99.5" r="32" strokeWidth="3" />
        <path d="M99.5 56C124.098 56 144 75.4964 144 99.5C144 123.504 124.098 143 99.5 143C74.9023 143 55 123.504 55 99.5C55 75.4964 74.9023 56 99.5 56Z" strokeWidth="2" />
        <circle cx="99.5" cy="99.5" r="55" strokeWidth="1" />
      </g>
    </svg>
  );
}

/** The rings alone, in the current text colour, for quiet places such as empty states. */
export function RingRings({ size = 40, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="40 40 120 120" fill="none" aria-hidden="true">
      <circle cx="99.5" cy="99.5" r="11.5" fill="currentColor" />
      <circle cx="99.5" cy="99.5" r="32" stroke="currentColor" strokeWidth="4" />
      <circle cx="99.5" cy="99.5" r="55" stroke="currentColor" strokeWidth="3" />
    </svg>
  );
}

export function Wordmark({ className, mark = true }: { className?: string; mark?: boolean }) {
  return (
    <span className={[styles.wordmark, className].filter(Boolean).join(' ')}>
      {mark ? <RingMark size={22} /> : null}
      <span>RING</span>
    </span>
  );
}
