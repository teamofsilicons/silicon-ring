import type { Account, Call, Device, Invitation, Platform, Voicemail } from './types';

/** The display name, or the id when there is none. */
export function label(account: Pick<Account, 'display_name' | 'id'> | null | undefined) {
  if (!account) return 'Someone';
  const name = account.display_name?.trim();
  return name || account.id;
}

export function hasName(account: Pick<Account, 'display_name'>) {
  return Boolean(account.display_name?.trim());
}

export function firstName(account: Account) {
  const name = account.display_name?.trim();
  return name ? name.split(/\s+/)[0] : account.id;
}

export function duration(ms: number | null | undefined) {
  if (ms === null || ms === undefined || ms < 0) return '';
  const total = Math.round(ms / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return hours ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
}

export function spokenDuration(ms: number | null | undefined) {
  if (!ms || ms < 1000) return '';
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  if (!minutes) return `${seconds}s`;
  return seconds ? `${minutes}m ${seconds}s` : `${minutes}m`;
}

const time = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
const weekday = new Intl.DateTimeFormat(undefined, { weekday: 'short' });
const day = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });
const dayYear = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', year: 'numeric' });
const full = new Intl.DateTimeFormat(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric', hour: 'numeric', minute: '2-digit' });

function startOfDay(date: Date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}

/** "14:05", "Yesterday", "Mon", "Oct 3" or "Oct 3, 2025", the way phone call logs show it. */
export function when(iso: string, now = new Date()) {
  const date = new Date(iso);
  const days = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000);
  if (days <= 0) return time.format(date);
  if (days === 1) return 'Yesterday';
  if (days < 7) return weekday.format(date);
  return date.getFullYear() === now.getFullYear() ? day.format(date) : dayYear.format(date);
}

export function clock(iso: string) {
  return time.format(new Date(iso));
}

export function fullDate(iso: string) {
  return full.format(new Date(iso));
}

/** The people on a call other than `me`, joined participants and invitees together. */
export function otherPeople(call: Call, meUuid: string | undefined) {
  const seen = new Set<string>();
  const people: Account[] = [];
  const add = (account: Account) => {
    if (account.uuid === meUuid || seen.has(account.uuid)) return;
    seen.add(account.uuid);
    people.push(account);
  };
  add(call.created_by);
  for (const participant of call.participants) add(participant.account);
  for (const invitation of call.invitations) add(invitation.to);
  return people;
}

/** My invitation on a call: the ringing one if there is one (I can be invited again after leaving), else the latest. */
export function myInvitation(call: Call, meUuid: string | undefined): Invitation | undefined {
  const mine = call.invitations.filter(invitation => invitation.to.uuid === meUuid);
  return mine.find(invitation => invitation.state === 'ringing')
    ?? mine.reduce<Invitation | undefined>((latest, invitation) => (!latest || Date.parse(invitation.created_at) >= Date.parse(latest.created_at) ? invitation : latest), undefined);
}

export function myParticipant(call: Call, meUuid: string | undefined) {
  return call.participants.find(participant => participant.account.uuid === meUuid);
}

export type Outcome = 'answered' | 'missed' | 'declined' | 'cancelled' | 'busy' | 'voicemail' | 'failed' | 'live' | 'ringing';

/** What the call meant for the viewer, for the call log. */
export function outcome(call: Call, meUuid: string | undefined): Outcome {
  if (call.state === 'ringing') return 'ringing';
  if (call.state === 'active' || call.state === 'voicemail') return call.state === 'active' ? 'live' : 'voicemail';
  const mine = myInvitation(call, meUuid);
  if (call.direction === 'incoming' && mine && mine.state !== 'accepted') {
    if (mine.state === 'declined') return 'declined';
    if (mine.state === 'busy') return 'busy';
    return 'missed';
  }
  if (call.answered_at) return 'answered';
  if (call.voicemail_id) return 'voicemail';
  switch (call.end_reason) {
    case 'declined': return 'declined';
    case 'busy': return 'busy';
    case 'cancelled': return 'cancelled';
    case 'failed':
    case 'server_restart': return 'failed';
    default: return 'missed';
  }
}

export function outcomeLabel(value: Outcome, direction: Call['direction']) {
  switch (value) {
    case 'answered': return direction === 'incoming' ? 'Incoming' : 'Outgoing';
    case 'missed': return direction === 'incoming' ? 'Missed' : 'No answer';
    case 'declined': return 'Declined';
    case 'cancelled': return 'Cancelled';
    case 'busy': return 'Busy';
    case 'voicemail': return 'Voicemail';
    case 'failed': return 'Failed';
    case 'live': return 'On call';
    case 'ringing': return 'Ringing';
  }
}

export function callDuration(call: Call) {
  if (!call.answered_at) return null;
  const end = call.ended_at ? Date.parse(call.ended_at) : Date.now();
  return Math.max(0, end - Date.parse(call.answered_at));
}

export function voicemailReason(voicemail: Voicemail) {
  if (voicemail.reason === 'declined') return voicemail.decline_reason ? `Declined · ${voicemail.decline_reason}` : 'Declined';
  return voicemail.reason === 'busy' ? 'Busy' : 'Missed';
}

const PLATFORM_NAMES: Record<Platform, string> = {
  ios: 'iPhone', ipados: 'iPad', macos: 'Mac', android: 'Android', windows: 'Windows', linux: 'Linux', web: 'Browser', cli: 'CLI',
};

export function deviceLabel(device: Pick<Device, 'name' | 'platform'> | null | undefined) {
  if (!device) return 'another device';
  return device.name?.trim() || (device.platform ? PLATFORM_NAMES[device.platform] : 'another device');
}

export function platformName(platform: Platform | null) {
  return platform ? PLATFORM_NAMES[platform] : 'Unknown';
}

/** A readable default name for this browser, such as "Chrome on macOS". */
export function browserDeviceName() {
  const agent = navigator.userAgent;
  const browser = /Edg\//.test(agent) ? 'Edge' : /Firefox\//.test(agent) ? 'Firefox' : /Chrome\//.test(agent) ? 'Chrome' : /Safari\//.test(agent) ? 'Safari' : 'Browser';
  const system = /iPhone|iPad|iPod/.test(agent) ? 'iOS' : /Android/.test(agent) ? 'Android' : /Mac OS X/.test(agent) ? 'macOS' : /Windows/.test(agent) ? 'Windows' : /Linux/.test(agent) ? 'Linux' : '';
  return system ? `${browser} on ${system}` : browser;
}

/** Normalises what someone typed into an actor reference, or null when it cannot be one. */
export function actorRef(input: string) {
  const value = input.trim().replace(/^@/, '').trim();
  if (!value) return null;
  const lower = value.toLowerCase();
  const handle = /^(c|si):([a-z0-9_.-]+)$/.exec(lower);
  if (handle) return lower;
  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(lower)) return lower;
  return null;
}

/** Counts Unicode code points, the way the server's text limits do. */
export function codePoints(text: string) {
  return Array.from(text).length;
}
