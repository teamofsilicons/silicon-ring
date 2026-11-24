/**
 * The notice that Ring records and transcribes calls, and which providers process the audio (as in the native apps:
 * `apple/RingKit/Calls/RecordingNotice.swift`). Each browser asks once, right after sign-in, and again before a call
 * until it has agreed, and remembers the answer in localStorage. Storage that can't be used (blocked, a private window)
 * keeps the answer for this page only.
 */

/** Raise this when the notice's wording changes enough that people should agree again. */
export const RECORDING_NOTICE_VERSION = 1;

const ACCEPTED_KEY = 'ring.recordingNotice.version';
/** The version last put to the person signed in here and answered with "Not now". */
const OFFERED_KEY = 'ring.recordingNotice.offered';

const remembered = { accepted: 0, offered: 0 };

function read(key: string) {
  try {
    const value = Number(localStorage.getItem(key));
    return Number.isFinite(value) ? value : 0;
  } catch {
    return 0;
  }
}

function write(key: string, value: number | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, String(value));
  } catch {
    // Remembered for this page only.
  }
}

/** Whether this browser agreed to the current notice. */
export function recordingNoticeAccepted() {
  return Math.max(remembered.accepted, read(ACCEPTED_KEY)) >= RECORDING_NOTICE_VERSION;
}

export function acceptRecordingNotice() {
  remembered.accepted = RECORDING_NOTICE_VERSION;
  write(ACCEPTED_KEY, RECORDING_NOTICE_VERSION);
}

/** Whether this sign-in has seen the current notice; the one after sign-in shows only until then. */
export function recordingNoticeOffered() {
  return Math.max(remembered.offered, read(OFFERED_KEY)) >= RECORDING_NOTICE_VERSION;
}

export function markRecordingNoticeOffered() {
  remembered.offered = RECORDING_NOTICE_VERSION;
  write(OFFERED_KEY, RECORDING_NOTICE_VERSION);
}

/** Signing out: the next sign-in shows the notice again, unless this browser has agreed. */
export function resetRecordingNoticeOffer() {
  remembered.offered = 0;
  write(OFFERED_KEY, null);
}

/** Whether a `storage` event from another tab of this browser is the agreement. */
export function isRecordingNoticeAgreement(event: StorageEvent) {
  return event.key === ACCEPTED_KEY && Number(event.newValue) >= RECORDING_NOTICE_VERSION;
}

/** What waits for the notice. */
export type RecordingNoticeAction =
  /** Right after sign-in, before any call: nothing waits. */
  | { kind: 'welcome' }
  /** Calling this id. */
  | { kind: 'place'; to: string }
  /** Answering this ring here. */
  | { kind: 'answer'; ringid: string; invitationId?: string }
  /** Moving this call here from another of the account's devices. */
  | { kind: 'switch'; ringid: string }
  /** Rejoin call: taking back a call this browser carried before the page reloaded. */
  | { kind: 'rejoin'; ringid: string };

/** The notice on screen, and the call action waiting for it. */
export interface RecordingNoticeRequest {
  id: number;
  action: RecordingNoticeAction;
}
