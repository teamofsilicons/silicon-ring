import { CallAudio } from './audio';
import { checkSessionContext, type RingSocket, type Session, type ConnectSettings } from './protocol';
let mobile = false;
export const isNativeMobile = () => mobile;
export const isNative = () => '__TAURI_INTERNALS__' in window;
export async function openNativeLogin(url: string) {
  const { openUrl } = await import('@tauri-apps/plugin-opener');
  await openUrl(url);
}
export async function listenNativeLogin(onUrl: (url: string) => void): Promise<() => void> {
  if (!isNative()) return () => {};
  const { getCurrent, onOpenUrl } = await import('@tauri-apps/plugin-deep-link');
  const stop = await onOpenUrl(urls => urls.forEach(onUrl));
  try { (await getCurrent())?.forEach(onUrl); }
  catch (error) { stop(); throw error; }
  return stop;
}
async function command(action: string, payload: Record<string, unknown> = {}) {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<any>('plugin:call-service|control', { payload: { action, ...payload } });
}
export async function configureNative(session: Session, settings: ConnectSettings) {
  if (!isNative()) return;
  checkSessionContext(session, settings);
  const { invoke } = await import('@tauri-apps/api/core');
  const result = await invoke<any>('plugin:call-service|configure', { payload: { ...settings, ...session } });
  mobile = result.mobile === true;
}
export async function logoutNative() { if (isNative()) await command('logout'); mobile = false; }
export async function restoreNative(): Promise<(Session & { url: string; realm: string; test_app_secret?: string }) | null> {
  if (!isNative()) return null;
  const stored = await command('restore');
  return stored.session_token && stored.device_id && stored.url ? stored : null;
}
export class PhoneAudio {
  private browser = new CallAudio();
  private nativeStream = '';
  private nativeMuted = false;
  private generation = 0;
  private poll?: ReturnType<typeof setInterval>;
  onMute: (muted: boolean) => void = () => {};
  set onLevel(callback: (level: number) => void) { this.browser.onLevel = callback; }
  get onLevel() { return this.browser.onLevel; }
  get streamId() { return mobile ? this.nativeStream : this.browser.streamId; }
  get muted() { return mobile ? this.nativeMuted : this.browser.muted; }
  get lastSeq() { return this.browser.lastSeq; }
  async start(api: RingSocket, ringid: string, device_id: string, voicemail_id?: string) {
    if (!mobile) return this.browser.start(api, ringid, device_id, voicemail_id);
    const generation = ++this.generation;
    clearInterval(this.poll);
    const result = await command('start', { ringid, voicemail_id });
    if (generation !== this.generation) return;
    this.nativeStream = result.stream_id; this.nativeMuted = false;
    clearInterval(this.poll);
    this.poll = setInterval(() => { void command('status').then(state => { if (generation !== this.generation) return; this.nativeStream = state.stream_id; this.nativeMuted = state.muted; this.onMute(state.muted); }).catch(() => {}); }, 1500);
  }
  async mute(api: RingSocket, muted: boolean) {
    if (!mobile) return this.browser.mute(api, muted);
    await command('mute', { muted }); this.nativeMuted = muted;
  }
  async stop(api?: RingSocket, voicemail = false) {
    ++this.generation;
    clearInterval(this.poll);
    if (!mobile) return this.browser.stop(api, voicemail);
    this.nativeStream = ''; this.onLevel(0);
    return command('stop');
  }
}
