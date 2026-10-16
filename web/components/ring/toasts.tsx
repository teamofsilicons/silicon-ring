'use client';

import { useEffect, useState, type CSSProperties } from 'react';
import { ToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import styles from './toasts.module.css';

// What a screen pins to the bottom edge (the tab bar and New call button, call controls, the ringing sheet's Accept and
// Decline) registers here, and toasts rise above the highest of it instead of covering it.
const pinned = new Set<HTMLElement>();
const changes = new Set<() => void>();

function changed() {
  for (const listener of changes) listener();
}

/** A ref for an element at the bottom of the screen that toasts must not cover. */
export function keepClearOfToasts(element: HTMLElement | null) {
  if (!element) return;
  pinned.add(element);
  changed();
  return () => {
    pinned.delete(element);
    changed();
  };
}

/** How far above the bottom edge toasts must stay: up to the top of the highest pinned element in view. */
function clearance() {
  const height = window.innerHeight;
  let top = height;
  for (const element of pinned) {
    const rect = element.getBoundingClientRect();
    // Hidden (display: none on this layout), scrolled out of view, or not at the bottom: nothing to keep clear of.
    if (!rect.width || !rect.height || rect.top >= height || rect.top < height * 0.4) continue;
    top = Math.min(top, rect.top);
  }
  return Math.round(height - top);
}

/** The app's toast stack, in a frame that ends where the screen's bottom controls begin. */
export function Toasts() {
  const [bottom, setBottom] = useState(0);

  useEffect(() => {
    let frame = 0;
    const observed = new Set<HTMLElement>();
    const resize = new ResizeObserver(() => schedule());
    function schedule() {
      frame ||= requestAnimationFrame(measure);
    }
    function measure() {
      frame = 0;
      for (const element of pinned) {
        if (!observed.has(element)) {
          resize.observe(element);
          observed.add(element);
        }
      }
      for (const element of observed) {
        if (!pinned.has(element)) {
          resize.unobserve(element);
          observed.delete(element);
        }
      }
      setBottom(clearance());
    }
    changes.add(schedule);
    window.addEventListener('resize', schedule);
    // Capturing, so scrolling inside a screen (such as the ringing sheet) counts too, and so does the end of an entrance
    // animation that slid the controls into place (a transform, which ResizeObserver doesn't see).
    window.addEventListener('scroll', schedule, { capture: true, passive: true });
    window.addEventListener('animationend', schedule, true);
    window.addEventListener('transitionend', schedule, true);
    schedule();
    return () => {
      cancelAnimationFrame(frame);
      changes.delete(schedule);
      resize.disconnect();
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, { capture: true });
      window.removeEventListener('animationend', schedule, true);
      window.removeEventListener('transitionend', schedule, true);
    };
  }, []);

  return (
    <div className={styles.frame} style={{ '--ring-toast-clearance': `${bottom}px` } as CSSProperties}>
      <ToastStack position="bottom-center" contained />
    </div>
  );
}
