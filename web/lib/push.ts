// Web Push for a closed or backgrounded Ring tab: the service worker (public/sw.js) shows the incoming-call
// notification. The subscription is registered with the server as this device's `webpush` token.

export type PushState = 'unsupported' | 'denied' | 'default' | 'granted';

export function pushSupported() {
  return typeof window !== 'undefined' && 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
}

export function pushPermission(): PushState {
  if (!pushSupported()) return 'unsupported';
  return Notification.permission === 'default' ? 'default' : Notification.permission === 'granted' ? 'granted' : 'denied';
}

let registration: Promise<ServiceWorkerRegistration | null> | null = null;

export function serviceWorker() {
  if (!('serviceWorker' in navigator)) return Promise.resolve(null);
  registration ??= navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch(() => null);
  return registration;
}

/**
 * The registration once its worker is active. Subscribing to push and showing notifications both fail on a worker that
 * is still installing, which is the case right after the first registration.
 */
async function activeServiceWorker(timeoutMs = 10_000): Promise<ServiceWorkerRegistration | null> {
  const registered = await serviceWorker();
  if (!registered || registered.active) return registered;
  const timeout = new Promise<null>(resolve => setTimeout(() => resolve(null), timeoutMs));
  return Promise.race([navigator.serviceWorker.ready, timeout]);
}

function keyBytes(base64url: string) {
  const padded = base64url.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(base64url.length / 4) * 4, '=');
  const raw = atob(padded);
  const bytes = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

function sameKey(subscription: PushSubscription, key: Uint8Array) {
  const current = subscription.options.applicationServerKey;
  if (!current) return false;
  const bytes = new Uint8Array(current);
  return bytes.length === key.length && bytes.every((byte, i) => byte === key[i]);
}

/**
 * Returns this browser's push subscription as the JSON string the API expects, creating it when notifications are
 * allowed. Returns null when they are not, so the caller can clear the device's tokens.
 */
export async function pushToken(vapidPublicKey: string | null): Promise<string | null> {
  if (!vapidPublicKey || pushPermission() !== 'granted') return null;
  const worker = await activeServiceWorker();
  if (!worker) return null;
  const key = keyBytes(vapidPublicKey);
  let subscription = await worker.pushManager.getSubscription();
  if (subscription && !sameKey(subscription, key)) {
    await subscription.unsubscribe();
    subscription = null;
  }
  subscription ??= await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
  return JSON.stringify(subscription.toJSON());
}

/** Asks for notification permission. Must run inside a user gesture. */
export async function requestNotifications(): Promise<PushState> {
  if (!('Notification' in window)) return 'unsupported';
  const result = await Notification.requestPermission();
  return result === 'granted' ? 'granted' : result === 'denied' ? 'denied' : 'default';
}

export async function unsubscribePush() {
  const worker = await serviceWorker();
  const subscription = await worker?.pushManager.getSubscription();
  await subscription?.unsubscribe();
}

/** Shows a notification from the page (used while the tab is hidden and push is not set up). */
export async function showPageNotification(title: string, options: NotificationOptions & { actions?: { action: string; title: string }[] }) {
  if (!('Notification' in window) || Notification.permission !== 'granted') return;
  const worker = await activeServiceWorker(3_000);
  if (worker) {
    try {
      await worker.showNotification(title, options);
      return;
    } catch {
      // Fall through to a page notification.
    }
  }
  const { actions: _actions, ...plain } = options;
  void _actions;
  const notification = new Notification(title, plain);
  // The tab already shows the call or voicemail; clicking brings it forward.
  notification.onclick = () => {
    window.focus();
    notification.close();
  };
}

export async function closeNotifications(tag: string) {
  const worker = await serviceWorker();
  const open = await worker?.getNotifications({ tag });
  for (const notification of open ?? []) notification.close();
}
