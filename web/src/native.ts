import { CallAudio } from './audio';
import type { RingSocket, Session } from './protocol';
let mobile = false;
export const isNativeMobile = () => mobile;
const native = () => '__TAURI_INTERNALS__' in window;
async function command(action: string, payload: Record<string, unknown> = {}) {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<any>('plugin:call-service|control', { payload: { action, ...payload } });
}
export async function configureNative(session: Session, settings: { url: string; org_id?: string; realm: string; test_app_secret?: string }) {
  if (!native()) return;
  const { invoke } = await import('@tauri-apps/api/core');
  const result = await invoke<any>('plugin:call-service|configure', { payload: { ...settings, ...session } });
  mobile = result.mobile === true;
}
export async function logoutNative() { if (native()) await command('logout'); mobile = false; }
export async function restoreNative(): Promise<(Session & { url: string; realm: string; test_app_secret?: string }) | null> {
  if (!native()) return null;
  const stored = await command('restore');
  return stored.session_token && stored.device_id && stored.url ? stored : null;
}
export class PhoneAudio {
  private browser = new CallAudio();
  private nativeStream = '';
  private nativeMuted = false;
  private poll?: ReturnType<typeof setInterval>;
  onMute: (muted: boolean) => void = () => {};
  set onLevel(callback: (level: number) => void) { this.browser.onLevel = callback; }
  get onLevel() { return this.browser.onLevel; }
  get streamId() { return mobile ? this.nativeStream : this.browser.streamId; }
  get muted() { return mobile ? this.nativeMuted : this.browser.muted; }
  get lastSeq() { return this.browser.lastSeq; }
  async start(api: RingSocket, ringid: string, device_id: string, voicemail_id?: string) {
    if (!mobile) return this.browser.start(api, ringid, device_id, voicemail_id);
    const result = await command('start', { ringid, voicemail_id });
    this.nativeStream = result.stream_id; this.nativeMuted = false;
    clearInterval(this.poll);
    this.poll = setInterval(() => { void command('status').then(state => { this.nativeStream = state.stream_id; this.nativeMuted = state.muted; this.onMute(state.muted); }).catch(() => {}); }, 1500);
  }
  async mute(api: RingSocket, muted: boolean) {
    if (!mobile) return this.browser.mute(api, muted);
    await command('mute', { muted }); this.nativeMuted = muted;
  }
  async stop(api?: RingSocket, voicemail = false) {
    clearInterval(this.poll);
    if (!mobile) return this.browser.stop(api, voicemail);
    this.nativeStream = ''; this.onLevel(0);
    return command('stop');
  }
}
