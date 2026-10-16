// Ring's service worker: shows incoming calls and voicemail from Web Push while no Ring tab is open.
// Payloads are the push objects in udd/api.md ("Push payloads"). Nothing is cached and nothing is reported anywhere.

const ICON = '/icons/icon-192.png';
const BADGE = '/icons/badge-96.png';

self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', event => event.waitUntil(self.clients.claim()));

function personName(person) {
  if (!person) return 'Someone';
  return (person.display_name && person.display_name.trim()) || person.id || 'Someone';
}

// Push people carry uuid, id and display_name; newer servers add kind ("carbon" or "silicon"), and uuid to participants.
function samePerson(a, b) {
  if (a.uuid && b.uuid) return a.uuid === b.uuid;
  if (a.id && b.id) return a.id === b.id;
  return personName(a) === personName(b);
}

function callingLine(from, others) {
  const lead = from.kind === 'silicon' ? 'Silicon calling you' : 'Calling you';
  const id = from.id ? ` · ${from.id}` : '';
  return others.length ? `${lead} with ${others.join(', ')}${id}` : `${lead}${id}`;
}

async function visibleClient() {
  const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
  return windows.find(client => client.visibilityState === 'visible') || null;
}

async function closeTagged(tag) {
  const open = await self.registration.getNotifications({ tag });
  for (const notification of open) notification.close();
}

async function handlePush(payload) {
  switch (payload.type) {
    case 'call.incoming': {
      // An open, visible Ring tab is already ringing in the page itself.
      if (await visibleClient()) return;
      const from = payload.from || {};
      // Participants are everyone already on the call, the caller among them.
      const others = (payload.participants || []).filter(person => person && !samePerson(person, from)).map(personName);
      await self.registration.showNotification(personName(from), {
        body: callingLine(from, others),
        tag: payload.ringid,
        renotify: true,
        requireInteraction: true,
        icon: from.pfp_url || ICON,
        badge: BADGE,
        timestamp: Date.now(),
        actions: [
          { action: 'answer', title: 'Answer' },
          { action: 'decline', title: 'Decline' },
        ],
        data: { kind: 'call', ringid: payload.ringid, invitation_id: payload.invitation_id, expires_at: payload.expires_at },
      });
      return;
    }
    case 'call.cancelled':
      await closeTagged(payload.ringid);
      return;
    case 'call.missed': {
      await closeTagged(payload.ringid);
      await self.registration.showNotification(`Missed call from ${personName(payload.from)}`, {
        body: payload.from ? payload.from.id : '',
        tag: payload.ringid,
        icon: (payload.from && payload.from.pfp_url) || ICON,
        badge: BADGE,
        data: { kind: 'missed', url: `/history?id=${encodeURIComponent(payload.ringid)}` },
      });
      return;
    }
    case 'voicemail.received':
      await self.registration.showNotification(`Voicemail from ${personName(payload.from)}`, {
        body: payload.transcript || 'New voicemail',
        tag: payload.voicemail_id,
        icon: (payload.from && payload.from.pfp_url) || ICON,
        badge: BADGE,
        data: { kind: 'voicemail', url: `/voicemail?id=${encodeURIComponent(payload.voicemail_id)}` },
      });
      return;
    default:
      return;
  }
}

self.addEventListener('push', event => {
  let payload = null;
  try {
    payload = event.data ? event.data.json() : null;
  } catch {
    payload = null;
  }
  if (!payload || typeof payload.type !== 'string') return;
  event.waitUntil(handlePush(payload));
});

// Browsers may replace a push subscription on their own. Register the new one as this device's token right away,
// or calls would stop reaching a closed Ring tab until the app is opened again.
self.addEventListener('pushsubscriptionchange', event => {
  event.waitUntil((async () => {
    let subscription = event.newSubscription || null;
    const key = event.oldSubscription && event.oldSubscription.options && event.oldSubscription.options.applicationServerKey;
    if (!subscription && key) {
      subscription = await self.registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    }
    if (!subscription) return;
    await fetch('/v1/devices/current', {
      method: 'PUT',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', 'X-Ring-Client': 'web' },
      body: JSON.stringify({ push: [{ kind: 'webpush', token: JSON.stringify(subscription.toJSON()) }] }),
    });
  })().catch(() => undefined));
});

async function openRing(url) {
  const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
  const ring = windows.find(client => new URL(client.url).origin === self.location.origin);
  if (ring) {
    ring.postMessage({ type: 'ring.open', url });
    if ('focus' in ring) await ring.focus().catch(() => undefined);
    return;
  }
  await self.clients.openWindow(url);
}

async function declineFromNotification(data) {
  const response = await fetch(`/v1/calls/${encodeURIComponent(data.ringid)}/decline`, {
    method: 'POST',
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', 'X-Ring-Client': 'web' },
    body: JSON.stringify({ invitation_id: data.invitation_id }),
  });
  return response.ok || response.status === 409 || response.status === 410;
}

self.addEventListener('notificationclick', event => {
  const notification = event.notification;
  const data = notification.data || {};
  notification.close();
  if (data.kind !== 'call') {
    event.waitUntil(openRing(data.url || '/'));
    return;
  }
  const call = `/call?id=${encodeURIComponent(data.ringid)}`;
  if (event.action === 'decline') {
    // Declining needs no window; fall back to the call screen if the request fails (for example, signed out).
    event.waitUntil(declineFromNotification(data).catch(() => false).then(ok => (ok ? undefined : openRing(`${call}&action=decline`))));
    return;
  }
  event.waitUntil(openRing(event.action === 'answer' ? `${call}&action=answer` : `${call}&action=open`));
});
