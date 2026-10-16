// Wire types for the Ring API v1 (udd/api.md, mirrored from crates/ring-proto). Times are RFC 3339 strings.

export type AccountKind = 'carbon' | 'silicon';

export interface Account {
  uuid: string;
  id: string;
  kind: AccountKind;
  display_name: string | null;
  pfp_url: string | null;
}

export interface AccountLookup {
  account: Account;
  on_ring: boolean;
}

export type Platform = 'ios' | 'ipados' | 'macos' | 'android' | 'windows' | 'linux' | 'web' | 'cli';
export type PushKind = 'apns_voip' | 'apns' | 'fcm' | 'webpush';

export interface Device {
  device_id: string;
  name: string | null;
  platform: Platform | null;
  app_version: string | null;
  created_at: string;
  last_seen_at: string | null;
  online: boolean;
  ring_enabled: boolean;
  current: boolean;
  push: { kind: PushKind; environment: 'sandbox' | 'production' | null }[];
}

export interface DeviceUpdate {
  name?: string;
  platform?: Platform;
  app_version?: string;
  ring_enabled?: boolean;
  push?: { kind: PushKind; token: string }[];
}

export interface Me {
  account: Account;
  device: Device;
}

export interface Voice {
  id: string;
  name: string;
  description: string;
}

export interface Meta {
  app_id: string;
  version: string;
  api_version: string;
  accounts_url: string;
  web_url: string;
  limits: { context: number; start: number; message: number; decline_reason: number; voicemail_text: number; greeting: number };
  ringing_seconds: number;
  voicemail_max_seconds: number;
  voices: Voice[];
  default_voice: string;
  webpush_public_key: string | null;
}

export type CallState = 'ringing' | 'active' | 'voicemail' | 'ended';
export type EndReason = 'completed' | 'cancelled' | 'declined' | 'missed' | 'busy' | 'failed' | 'server_restart';
export type RecordingStatus = 'none' | 'recording' | 'processing' | 'ready' | 'failed';
export type InvitationState = 'ringing' | 'accepted' | 'declined' | 'missed' | 'cancelled' | 'busy';

export interface DeviceRef {
  device_id: string;
  name: string | null;
  platform: Platform | null;
}

export interface Participant {
  account: Account;
  state: 'joined' | 'left';
  joined_at: string;
  left_at: string | null;
  invited_by: Account | null;
  device: DeviceRef | null;
  representative: { status: 'starting' | 'live' | 'failed' | 'ended'; voice: string } | null;
  media_identity: string | null;
}

export interface Invitation {
  invitation_id: string;
  ringid: string;
  from: Account;
  to: Account;
  state: InvitationState;
  reason: string | null;
  created_at: string;
  expires_at: string;
  answered_at: string | null;
  silenced: boolean;
  call_uuid: string;
}

export interface Call {
  ringid: string;
  state: CallState;
  created_at: string;
  answered_at: string | null;
  ended_at: string | null;
  end_reason: EndReason | null;
  created_by: Account;
  direction: 'outgoing' | 'incoming';
  participants: Participant[];
  invitations: Invitation[];
  recording: { status: RecordingStatus; duration_ms: number | null };
  voicemail_id: string | null;
}

export interface MediaGrant {
  url: string;
  token: string;
  room: string;
  identity: string;
  expires_at: string;
}

export interface CreatedCall {
  call: Call;
  invitation: Invitation;
  media: MediaGrant | null;
}

export interface CallWithMedia {
  call: Call;
  media: MediaGrant | null;
}

export interface Page<T> {
  items: T[];
  next_cursor: string | null;
}

export interface RecentPerson {
  account: Account;
  last_call_at: string;
  last_ringid: string;
  calls: number;
}

export type EntryKind =
  | 'call_started' | 'answered' | 'joined' | 'left' | 'invited' | 'declined' | 'missed' | 'busy'
  | 'speech' | 'delegation' | 'thinking' | 'commentary' | 'voicemail' | 'call_ended' | 'note';

export interface TranscriptEntry {
  seq: number;
  at: string;
  offset_ms: number | null;
  kind: EntryKind;
  actor: Account | null;
  text: string | null;
  /** For `speech`, `segment_id` names the transcript.partial events the entry finalises (absent from older servers). */
  data: Record<string, unknown>;
}

export interface TranscriptPage {
  items: TranscriptEntry[];
  next_after_seq: number;
}

export interface Recording {
  status: RecordingStatus;
  duration_ms: number | null;
  url?: string | null;
  expires_at?: string | null;
  mime_type?: string | null;
}

export type VoicemailReason = 'busy' | 'declined' | 'missed';

export interface Voicemail {
  voicemail_id: string;
  ringid: string;
  from: Account;
  to: Account;
  reason: VoicemailReason;
  decline_reason: string | null;
  created_at: string;
  duration_ms: number | null;
  audio_status: 'recording' | 'processing' | 'ready' | 'failed';
  transcript: string | null;
  transcript_status: 'pending' | 'ready' | 'failed';
  read: boolean;
  audio_url?: string | null;
}

export interface Greetings {
  busy: string | null;
  declined: string | null;
  missed: string | null;
}

export interface Settings {
  voice: string;
  representative: { default_context: string; context_mode: 'append' | 'prepend' | 'overwrite'; context_required: boolean };
  voicemail: { enabled: boolean; greetings: Greetings };
  revision: number;
}

export interface SettingsPatch {
  voice?: string;
  voicemail?: { enabled?: boolean; greetings?: Partial<Greetings> };
}

export type RingingStoppedReason = 'accepted_elsewhere' | 'declined' | 'silenced' | 'cancelled' | 'missed';

/** A server frame on /v1/ws. `at` is the server's clock when it sent the frame. */
export type RealtimeEvent = { at?: string } & (
  | { type: 'ready'; data: { account: Account; device_id: string } }
  | { type: 'call.incoming'; data: { call: Call; invitation: Invitation } }
  | { type: 'call.updated'; data: { call: Call } }
  // `from` (the caller) and the handoff's `device` are absent from older servers.
  | { type: 'call.ringing_stopped'; data: { ringid: string; invitation_id: string; reason: RingingStoppedReason; from?: Account | null } }
  | { type: 'call.handoff'; data: { ringid: string; device_id: string; device?: DeviceRef | null } }
  | { type: 'call.ended'; data: { call: Call } }
  | { type: 'transcript.entry'; data: { ringid: string; entry: TranscriptEntry } }
  | { type: 'transcript.partial'; data: { ringid: string; speaker: Account; segment_id: string; text: string } }
  | { type: 'voicemail.received'; data: { voicemail: Voicemail } }
  | { type: 'voicemail.updated'; data: { voicemail: Voicemail } }
  | { type: 'device.updated'; data: { device: Device } }
  | { type: 'session.revoked'; data: Record<string, never> }
  | { type: 'pong'; data?: unknown }
  | { type: 'delegation.created'; data: unknown }
);
