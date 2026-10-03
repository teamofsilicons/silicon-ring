import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import { RingTone } from './audio';
import { PhoneAudio, configureNative, logoutNative, restoreNative, isNativeMobile } from './native';
import { RingSocket, type Call, type Session, type ConnectSettings, displayActor, downloadAsset, uploadAsset, readTranscriptSince, coalesceTranscript, errorMessage, isActor, normalizeActor, normalizeRealm, bindSession, sessionToRestore, defaultSocketUrl } from './protocol';

type IconName = 'phone' | 'history' | 'voicemail' | 'devices' | 'settings' | 'arrow' | 'plus' | 'search' | 'close' | 'chevron' | 'mic' | 'muted' | 'end' | 'download' | 'check' | 'logout' | 'user' | 'bell' | 'spark' | 'refresh' | 'play' | 'back';
const paths: Record<IconName, string> = {
  phone: 'M22 16.92v3a2 2 0 0 1-2.18 2 19.8 19.8 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.8 19.8 0 0 1 2.12 4.2 2 2 0 0 1 4.1 2h3a2 2 0 0 1 2 1.72c.12.96.36 1.9.7 2.8a2 2 0 0 1-.45 2.1L8.1 9.9a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.1-.45c.9.34 1.84.58 2.8.7A2 2 0 0 1 22 16.92z',
  history: 'M3 11a9 9 0 1 1 2.4 7M3 4v7h7m2-4v5l3 2', voicemail: 'M8 17h8M8 13a4 4 0 1 1-8 0 4 4 0 0 1 8 0Zm16 0a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z',
  devices: 'M3 3h14v12H3zM7 19h6m-3-4v4m8-9h4v11h-7V10z', settings: 'm9 3 1-1h4l1 3 3 1 3 3-1 3 1 3-3 3-3 1-1 3h-4l-1-3-3-1-3-3 1-3-1-3 3-3 3-1z M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0',
  arrow: 'M7 17 17 7M7 7h10v10', plus: 'M12 5v14M5 12h14', search: 'M21 21l-4.5-4.5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0', close: 'm6 6 12 12M6 18 18 6', chevron: 'm9 5 7 7-7 7',
  mic: 'M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3zM5 10v2a7 7 0 0 0 14 0v-2M12 19v3m-4 0h8', muted: 'M9 5a3 3 0 0 1 6 0v7M9 9v3a3 3 0 0 0 4 2.8M5 10v2a7 7 0 0 0 12 4.9M19 10v2M12 19v3m-4 0h8M2 2l20 20', end: 'M3 16v-4c5-5 13-5 18 0v4h-5v-4M8 12v4H3', download: 'M12 3v12m-5-5 5 5 5-5M5 17v4h14v-4', check: 'm5 12 4 4L19 6', logout: 'M9 3H3v18h6m6-14 5 5-5 5M8 12h12', user: 'M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0M4 21v-2a8 8 0 0 1 16 0v2', bell: 'M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9M10 21h4', spark: 'm12 3 2.5 6.5L21 12l-6.5 2.5L12 21l-2.5-6.5L3 12l6.5-2.5z', refresh: 'M20 7a9 9 0 1 0 1 8M20 3v5h-5', play: 'm8 4 12 8-12 8z', back: 'M20 12H4m7-7-7 7 7 7',
};
function Icon(props: { name: IconName; size?: number }) { return <svg width={props.size || 20} height={props.size || 20} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={paths[props.name]} /></svg>; }
function Logo() { return <span class="logo-mark"><svg viewBox="0 0 50 50" fill="none" aria-hidden="true"><ellipse cx="25" cy="25" rx="20" ry="11" transform="rotate(-38 25 25)" /><ellipse cx="25" cy="25" rx="11" ry="20" transform="rotate(-38 25 25)" /></svg></span>; }
function Avatar(props: { actor?: string; name?: string; large?: boolean; photo?: string }) { return <span class={`avatar ${props.actor?.startsWith('si:') ? 'silicon' : ''} ${props.large ? 'large' : ''}`}><Show when={props.photo} fallback={props.actor?.startsWith('si:') ? <Icon name="spark" size={props.large ? 35 : 22} /> : (props.name || displayActor(props.actor) || '?').slice(0, 2).toUpperCase()}><img src={props.photo} alt="" /></Show></span>; }
function Empty(props: { icon: IconName; title: string; children: any }) { return <div class="empty"><span class="empty-icon"><Icon name={props.icon} size={27} /></span><h3>{props.title}</h3><p>{props.children}</p></div>; }
const formatTime = (value: string) => new Date(value).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
const formatDate = (value: string) => new Date(value).toLocaleDateString([], { month: 'short', day: 'numeric' });
const duration = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
const storage = { read: <T,>(key: string, fallback: T): T => { try { return JSON.parse(sessionStorage.getItem(key) || 'null') || fallback; } catch { return fallback; } }, write: (key: string, value: unknown) => sessionStorage.setItem(key, JSON.stringify(value)) };
const defaultServer = import.meta.env.VITE_RING_SERVER || defaultSocketUrl(location.hostname, '__TAURI_INTERNALS__' in window, import.meta.env.DEV);

export default function App() {
  const api = new RingSocket(), audio = new PhoneAudio(), tone = new RingTone();
  const saved = storage.read<Session | null>('ring.session', null);
  const [session, setSession] = createSignal<Session | null>(saved);
  const [status, setStatus] = createSignal('offline');
  const [page, setPage] = createSignal('calls');
  const [calls, setCalls] = createSignal<Call[]>([]);
  const [voicemails, setVoicemails] = createSignal<any[]>([]);
  const [devices, setDevices] = createSignal<any[]>([]);
  const [profile, setProfile] = createSignal<any>({});
  const [config, setConfig] = createSignal<any>({});
  const [configReady, setConfigReady] = createSignal(false);
  const [selected, setSelected] = createSignal('');
  const [transcript, setTranscript] = createSignal<any[]>([]);
  const [filter, setFilter] = createSignal('all');
  const [search, setSearch] = createSignal('');
  const [error, setError] = createSignal('');
  const [notice, setNotice] = createSignal('');
  const [busy, setBusy] = createSignal(false);
  const [dialOpen, setDialOpen] = createSignal(false);
  const [target, setTarget] = createSignal('');
  const [inviteMode, setInviteMode] = createSignal(false);
  const [declineOpen, setDeclineOpen] = createSignal(false);
  const [declineReason, setDeclineReason] = createSignal('');
  const [token, setToken] = createSignal('');
  const storedConnection = storage.read<ConnectSettings | null>('ring.connection', null);
  const settings = storedConnection || { url: defaultServer, realm: 'production', org_id: '', test_app_secret: '' };
  const [server, setServer] = createSignal(settings.url);
  const [org, setOrg] = createSignal(settings.org_id || '');
  const [realm, setRealm] = createSignal(settings.realm);
  const [testSecret, setTestSecret] = createSignal(settings.test_app_secret || '');
  const [advanced, setAdvanced] = createSignal(false);
  const [name, setName] = createSignal('');
  const [greeting, setGreeting] = createSignal('');
  const [telemetry, setTelemetry] = createSignal(true);
  const [voicemailEnabled, setVoicemailEnabled] = createSignal(true);
  const [muted, setMuted] = createSignal(false);
  const [audioReady, setAudioReady] = createSignal(false);
  const [audioLevel, setAudioLevel] = createSignal(0);
  const [now, setNow] = createSignal(Date.now());
  const [vmDraft, setVmDraft] = createSignal('');
  const [vmStarted, setVmStarted] = createSignal(0);
  const [vmAudio, setVmAudio] = createSignal('');
  const [openedVm, setOpenedVm] = createSignal<any>(null);
  const [profilePhoto, setProfilePhoto] = createSignal('');
  const [people, setPeople] = createSignal<Record<string, any>>({});
  const [photos, setPhotos] = createSignal<Record<string, string>>({});
  let ownPhotoId = ''; const loadingPeople = new Set<string>();
  let settingsDirty = false; let voicemailRequest: { ringid: string; id: string } | undefined;
  let audioCall = '', ringtoneCall = '', refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let transcriptView: { ringid: string; owner: Session; after: number; again?: boolean; pending?: Promise<void> } | undefined;
  let alive = true;
  const me = () => session()?.actor || session()?.actor_id || '';
  const activeCall = createMemo(() => calls().find(call => ['active', 'connecting'].includes(call.state) && call.participants?.some(p => p.actor === me() && !p.left_at)));
  const selectedCall = createMemo(() => calls().find(call => call.ringid === selected()));
  const incoming = (call?: Call) => !!call?.invitations?.some(i => i.target === me() && ['pending', 'ringing'].includes(i.state));
  const other = (call: Call) => call.participants?.find(p => p.actor !== me())?.actor || (call.caller === me() ? call.target : call.caller);
  const otherName = (call: Call) => people()[other(call)]?.display_name || call.participants?.find(p => p.actor === other(call))?.display_name || displayActor(other(call));
  const visibleCalls = createMemo(() => calls().filter(call => (filter() !== 'missed' || (call.caller !== me() && ['missed', 'timeout', 'declined'].includes(call.outcome || call.invitations?.find(i => i.target === me())?.state || ''))) && `${otherName(call)} ${other(call)}`.toLowerCase().includes(search().toLowerCase())));
  const recentContacts = createMemo(() => [...new Map(calls().map(call => [other(call), { actor: other(call), name: otherName(call) }])).values()].filter(x => x.actor).slice(0, 5));
  const unread = () => voicemails().filter(vm => !vm.read).length;
  const ownParticipant = () => activeCall()?.participants?.find(p => p.actor === me() && !p.left_at);
  const callHere = () => ownParticipant()?.device_id === session()?.device_id;
  const telemetryAllowed = createMemo(() => !!session() && configReady() && config()['telemetry.enabled'] !== false);
  function track(source: 'web_analytics' | 'web_events', step: 'page_view' | 'call_action' | 'connection' | 'change_settings', progress: string, trace_id = crypto.randomUUID(), duration_ms?: number) {
    if (!telemetryAllowed() || !api.ready) return;
    void api.request('telemetry.record', { source, step, progress, trace_id, ...(duration_ms === undefined ? {} : { duration_ms }) }).catch(() => {});
  }

  api.onStatus = setStatus;
  api.onExpired = () => { setSession(null); clearTranscript(); setConfigReady(false); sessionStorage.removeItem('ring.session'); setError('Your session expired. Sign in with a new IAM token.'); };
  audio.onLevel = setAudioLevel; audio.onMute = setMuted;
  const run = async (task: () => Promise<void>, step?: 'call_action' | 'change_settings') => {
    setError(''); setBusy(true); const started = performance.now(), trace = crypto.randomUUID();
    if (step) track('web_events', step, 'started', trace);
    try { await task(); if (step) track('web_events', step, 'succeeded', trace, Math.round(performance.now() - started)); }
    catch (e) { if (step) track('web_events', step, 'failed', trace, Math.round(performance.now() - started)); setError(errorMessage(e)); }
    finally { setBusy(false); }
  };
  async function refresh() {
    if (!api.ready || !session()) return;
    const results = await Promise.allSettled([api.request('calls.list', { limit: 200 }), api.request('voicemail.list', { unread: false }), api.request('devices.list'), api.request('profile.get'), api.request('config.get', { scope: 'actor' })]);
    if (!alive) return;
    const [c, v, d, p, conf] = results;
    if (c.status === 'fulfilled') { setCalls(c.value.items || []); for (const call of c.value.items || []) void loadPerson(other(call)); }
    if (v.status === 'fulfilled') setVoicemails(v.value.items || []);
    if (d.status === 'fulfilled') setDevices(d.value.items || []);
    if (p.status === 'fulfilled') { setProfile(p.value); if (!settingsDirty) setName(p.value.display_name || displayActor(me())); if (p.value.photo_asset_id && ownPhotoId !== p.value.photo_asset_id) { ownPhotoId = p.value.photo_asset_id; void downloadAsset(api, ownPhotoId).then(blob => { if (profilePhoto()) URL.revokeObjectURL(profilePhoto()); setProfilePhoto(URL.createObjectURL(blob)); }).catch(() => {}); } }
    if (conf.status === 'fulfilled') { const values = conf.value.effective || conf.value.values || conf.value; setConfig(values); setConfigReady(true); if (!settingsDirty) { setGreeting(values['voicemail.greetings.declined']?.text || ''); setTelemetry(values['telemetry.enabled'] !== false); setVoicemailEnabled(values['voicemail.enabled'] !== false); } }
    if (c.status === 'rejected') throw c.reason;
    if (selected()) await loadTranscript(selected());
  }
  async function loadPerson(actor: string) {
    if (!actor || people()[actor] || loadingPeople.has(actor)) return;
    loadingPeople.add(actor);
    try { const person = await api.request('profile.get', { actor }); setPeople(current => ({ ...current, [actor]: person })); if (person.photo_asset_id) { const blob = await downloadAsset(api, person.photo_asset_id); setPhotos(current => ({ ...current, [actor]: URL.createObjectURL(blob) })); } } catch {} finally { loadingPeople.delete(actor); }
  }
  function clearTranscript() { transcriptView = undefined; setTranscript([]); }
  async function loadTranscript(ringid: string) {
    const owner = session();
    if (!owner || selected() !== ringid) return;
    if (transcriptView?.ringid !== ringid || transcriptView.owner !== owner) {
      clearTranscript(); transcriptView = { ringid, owner, after: 0 };
    }
    const view = transcriptView;
    if (view.pending) { view.again = true; return view.pending; }
    view.pending = (async () => {
      do {
        view.again = false;
        const result = await readTranscriptSince(api, ringid, view.after);
        if (!alive || transcriptView !== view || session() !== owner || selected() !== ringid) return;
        setTranscript(previous => coalesceTranscript([...previous, ...result.items]));
        view.after = result.latest_seq;
      } while (view.again);
    })().catch(e => {
      if (transcriptView === view && session() === owner && selected() === ringid) setError(`Transcript: ${errorMessage(e)}`);
    }).finally(() => { view.pending = undefined; });
    return view.pending;
  }
  createEffect(() => { const ringid = selected(); if (ringid && session()) void loadTranscript(ringid); else clearTranscript(); });
  async function connect() {
    const connection = { url: server(), realm: normalizeRealm(realm()), org_id: org().trim(), test_app_secret: testSecret() };
    setRealm(connection.realm); setOrg(connection.org_id);
    storage.write('ring.connection', connection);
    await api.connect(connection);
    return connection;
  }
  async function login() {
    await run(async () => {
      setConfigReady(false); api.disconnect(); const connection = await connect();
      let result: Session;
      try { result = bindSession(await api.request<Session>('auth.login', { token: token().trim() }), connection); }
      catch (error) { api.disconnect(); throw error; }
      api.session = result; await configureNative(result, { url: server(), realm: realm(), org_id: org(), test_app_secret: testSecret() }); setSession(result); storage.write('ring.session', result); setToken('');
      await api.request('events.subscribe', {}); await refresh(); window.scrollTo(0, 0);
    });
  }
  async function logout() {
    await run(async () => {
      await audio.stop(api).catch(() => {});
      try { if (api.ready) await api.request('auth.logout', {}); }
      finally { try { await logoutNative(); } finally { tone.stop(); api.disconnect(); sessionStorage.removeItem('ring.session'); setSession(null); clearTranscript(); setConfigReady(false); setCalls([]); setVoicemails([]); setSelected(''); setAudioReady(false); audioCall = ''; setProfile({}); setName(''); setConfig({}); setPeople({}); if (profilePhoto()) URL.revokeObjectURL(profilePhoto()); setProfilePhoto(''); ownPhotoId = ''; Object.values(photos()).forEach(url => URL.revokeObjectURL(url)); setPhotos({}); } }
    });
  }
  function selectCall(call: Call) { setSelected(call.ringid); setDeclineOpen(false); }
  function openDial(invite = false) { setInviteMode(invite); setTarget(''); setDialOpen(true); }
  async function dial() {
    if (!isActor(target())) { setError('Use a Carbon ID (c:name) or Silicon ID (si:name).'); return; }
    await run(async () => {
      const result = await api.request(inviteMode() ? 'calls.invite' : 'calls.init', { target: normalizeActor(target()), ...(inviteMode() ? { ringid: selected() } : { device_id: session()?.device_id }) });
      setDialOpen(false); await refresh(); setSelected(result.ringid || selected());
    }, 'call_action');
  }
  async function callAction(method: string, extras = {}) {
    await run(async () => { await api.request(method, { ringid: selected(), ...extras }); setDeclineOpen(false); await refresh(); }, 'call_action');
  }
  async function startAudio(call: Call) {
    if (audioCall === call.ringid || vmDraft()) return;
    audioCall = call.ringid;
    try { await audio.start(api, call.ringid, session()!.device_id); setAudioReady(true); setMuted(false); }
    catch (e) { audioCall = ''; setAudioReady(false); setError(`Microphone: ${errorMessage(e)}`); }
  }
  async function moveHere() {
    await run(async () => { const call = activeCall()!; await startAudio(call); if (!audioReady()) return; try { await api.request('calls.handoff', { ringid: call.ringid, to_device_id: session()!.device_id }); await refresh(); } catch (e) { audioCall = ''; setAudioReady(false); await audio.stop(api); throw e; } }, 'call_action');
  }
  async function saveSettings() {
    await run(async () => { await api.request('profile.update', { display_name: name().trim() }); await api.request('config.set', { scope: 'actor', values: { 'voicemail.enabled': voicemailEnabled(), 'telemetry.enabled': telemetry(), ...(greeting().trim() ? { 'voicemail.greetings.declined': { text: greeting().trim() } } : {}) } }); if (!greeting().trim()) await api.request('config.reset', { scope: 'actor', keys: ['voicemail.greetings.declined'] }); settingsDirty = false; await refresh(); setNotice('Your changes are saved.'); }, 'change_settings');
  }
  async function photoUpload(file?: File) {
    if (!file) return;
    await run(async () => {
      if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) throw new Error('Choose a PNG, JPEG, or WebP photo.');
      if (file.size > 5 * 1024 * 1024) throw new Error('Choose a photo smaller than 5 MB.');
      const asset_id = await uploadAsset(api, file, 'profile_photo');
      await api.request('profile.update', { photo_asset_id: asset_id });
      ownPhotoId = asset_id;
      if (profilePhoto()) URL.revokeObjectURL(profilePhoto()); setProfilePhoto(URL.createObjectURL(file)); await refresh();
    });
  }
  async function saveRecording(call: Call) {
    await run(async () => { const recording = await api.request('recordings.get', { ringid: call.ringid }); if (recording.status !== 'ready' || !(recording.audio_asset_id || recording.asset_id)) throw new Error(`Recording is ${recording.status || 'not available yet'}.`); const blob = await downloadAsset(api, recording.audio_asset_id || recording.asset_id); const url = URL.createObjectURL(blob); const a = document.createElement('a'); a.href = url; a.download = `${call.ringid}.wav`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000); });
  }
  async function openVoicemail(vm: any) {
    await run(async () => { const result = await api.request('voicemail.get', { voicemail_id: vm.voicemail_id }); setOpenedVm(result); if (vmAudio()) URL.revokeObjectURL(vmAudio()); setVmAudio(''); if (result.audio_asset_id || result.asset_id) setVmAudio(URL.createObjectURL(await downloadAsset(api, result.audio_asset_id || result.asset_id))); });
  }
  async function beginVoicemail() {
    await run(async () => {
      if (me().startsWith('si:')) throw new Error('Use the Ring CLI to leave voicemail in your representative’s voice.');
      await audio.stop(api); setAudioReady(false); audioCall = '';
      setNotice('Preparing the greeting…'); if (voicemailRequest?.ringid !== selected()) voicemailRequest = { ringid: selected(), id: crypto.randomUUID() }; const draft = await api.request('voicemail.begin', { ringid: selected(), format: 'audio' }, voicemailRequest.id); voicemailRequest = undefined; setNotice(''); setVmDraft(draft.voicemail_id);
      try {
        if (draft.greeting_asset_id || draft.greeting?.asset_id) { const blob = await downloadAsset(api, draft.greeting_asset_id || draft.greeting.asset_id); const url = URL.createObjectURL(blob); try { const player = new Audio(url); await new Promise<void>((resolve, reject) => { player.onended = () => resolve(); player.onerror = () => reject(new Error('The greeting could not play.')); player.play().catch(reject); }); } finally { URL.revokeObjectURL(url); } }
        if (draft.greeting?.text && !(draft.greeting_asset_id || draft.greeting?.asset_id)) setNotice(`Greeting: ${draft.greeting.text}`);
        const ctx = new AudioContext(); await ctx.resume(); const beep = ctx.createOscillator(); const volume = ctx.createGain(); volume.gain.value = 0.08; beep.frequency.value = 880; beep.connect(volume); volume.connect(ctx.destination); beep.start(); beep.stop(ctx.currentTime + 0.2); await new Promise<void>(resolve => { beep.onended = () => { void ctx.close(); resolve(); }; });
        await audio.start(api, selected(), session()!.device_id, draft.voicemail_id); setVmStarted(Date.now()); setAudioReady(true);
      } catch (e) { await api.request('voicemail.abort', { voicemail_id: draft.voicemail_id }); setVmDraft(''); throw e; }
    });
  }
  async function finishVoicemail(commit: boolean) {
    await run(async () => { const persisted = await audio.stop(api, true); setAudioReady(false); if (commit && (persisted?.complete === false || persisted?.persisted === false)) throw new Error('Some audio did not arrive. Discard this draft and record again.'); await api.request(commit ? 'voicemail.commit' : 'voicemail.abort', { voicemail_id: vmDraft() }); setVmDraft(''); setNotice(commit ? 'Your voicemail has been sent.' : 'Recording discarded.'); await refresh(); });
  }
  async function notifications() {
    await run(async () => { if (!('Notification' in window)) throw new Error('This browser does not support notifications.'); const permission = await Notification.requestPermission(); setNotice(permission === 'granted' ? 'Notifications enabled while Ring is open.' : 'Notifications are blocked. You can allow them in browser settings.'); });
  }
  createEffect(() => { page(); if (telemetryAllowed()) track('web_analytics', 'page_view', 'viewed'); });
  createEffect(() => { const state = status(); if (telemetryAllowed() && ['connected', 'reconnecting', 'offline'].includes(state)) track('web_events', 'connection', state); });
  createEffect(() => {
    const call = activeCall(); const here = callHere();
    if (call && here && !me().startsWith('si:') && !vmDraft()) void startAudio(call);
    else if (audioCall && (!call || !here) && !vmDraft()) { audioCall = ''; setAudioReady(false); void audio.stop(api); }
  });
  createEffect(() => {
    const ringing = calls().find(call => (call.state === 'ringing' && call.caller === me() || incoming(call)) && !call.invitations?.some(i => i.target === me() && (i as any).silenced));
    if (ringing && ringtoneCall !== ringing.ringid) { ringtoneCall = ringing.ringid; if (!(isNativeMobile() && incoming(ringing))) void tone.start(incoming(ringing)).catch(() => {}); }
    else if (!ringing) { ringtoneCall = ''; tone.stop(); }
  });
  createEffect(() => { if (dialOpen()) queueMicrotask(() => document.getElementById('call-target')?.focus()); });
  onMount(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    const keyboard = (event: KeyboardEvent) => {
      if (!dialOpen() && !openedVm()) return;
      if (event.key === 'Escape') { setDialOpen(false); setOpenedVm(null); }
      if (event.key === 'Tab') { const elements = Array.from(document.querySelectorAll<HTMLElement>('.modal button:not([disabled]), .modal input, .modal audio, .modal textarea')); const first = elements[0], last = elements[elements.length - 1]; if (event.shiftKey && document.activeElement === first) { last?.focus(); event.preventDefault(); } else if (!event.shiftKey && document.activeElement === last) { first?.focus(); event.preventDefault(); } }
    };
    document.addEventListener('keydown', keyboard);
    const unsubscribe = api.subscribe(event => {
      if (event.type === 'connection.lost') { audioCall = ''; setAudioReady(false); tone.stop(); return; }
      if (event.type === 'stream.error') { setError(event.data.error?.message || 'The audio stream was interrupted.'); if (event.data.stream_id === audio.streamId) { audioCall = ''; setAudioReady(false); void audio.stop(); } return; }
      if (event.type.startsWith('media.') || event.type.startsWith('assets.')) return;
      if (event.type === 'call.silenced') { tone.stop(); ringtoneCall = event.data.ringid; }
      if (event.type === 'call.incoming') { setSelected(event.data.ringid); setPage('calls'); if ('Notification' in window && Notification.permission === 'granted' && document.visibilityState !== 'visible') new Notification('Incoming Ring call', { body: 'Open Ring to see who is calling.', icon: '/ring.svg' }); }
      clearTimeout(refreshTimer); refreshTimer = setTimeout(() => { void refresh().catch(e => setError(errorMessage(e))); }, 150);
    });
    void run(async () => {
      const restored = saved?.url ? null : await restoreNative();
      const previous = sessionToRestore(saved, restored);
      if (restored && previous === restored && !storedConnection) { setServer(restored.url); setRealm(restored.realm); setOrg(restored.org_id); setTestSecret(restored.test_app_secret || ''); }
      if (!previous) return;
      api.session = previous; await connect(); setSession(api.session || null);
      if (api.session) { storage.write('ring.session', api.session); await configureNative(api.session, { url: server(), realm: realm(), org_id: org(), test_app_secret: testSecret() }); }
      await refresh();
    });
    onCleanup(() => { alive = false; document.removeEventListener('keydown', keyboard); clearInterval(timer); clearTimeout(refreshTimer); unsubscribe(); tone.stop(); void audio.stop(); api.disconnect(); if (vmAudio()) URL.revokeObjectURL(vmAudio()); if (profilePhoto()) URL.revokeObjectURL(profilePhoto()); Object.values(photos()).forEach(url => URL.revokeObjectURL(url)); });
  });

  return <div class="app-shell">
    <aside class="sidebar">
      <a class="brand" href="#" onClick={e => { e.preventDefault(); setPage('calls'); }}><Logo /><span>ring<span class="brand-dot">.</span></span></a>
      <div class="workspace"><span class="workspace-icon"><Icon name="spark" size={15} /></span><div><strong>{session()?.org_id || 'Silicon Ring'}</strong><small>{session() ? 'Your shared workspace' : 'For carbons & silicons'}</small></div><span class="workspace-dot" /></div>
      <span class="nav-label">WORKSPACE</span>
      <nav aria-label="Main navigation"><For each={[{ id: 'calls', icon: 'phone', label: 'Calls' }, { id: 'voicemail', icon: 'voicemail', label: 'Voicemail' }, { id: 'devices', icon: 'devices', label: 'My devices' }] as const}>{item => <button classList={{ active: page() === item.id }} onClick={() => { setPage(item.id); setSelected(''); }}><Icon name={item.icon} /><span>{item.label}</span><Show when={item.id === 'voicemail' && unread() > 0}><span class="nav-count">{unread()}</span></Show><Show when={page() === item.id}><span class="active-dot" /></Show></button>}</For></nav>
      <div class="sidebar-bottom"><div class="line-art" aria-hidden="true"><span /><span /><span /></div><div class="sidebar-note"><span class="tiny-dot" /> A little closer.<p>One line for every kind of mind.</p></div><button classList={{ 'settings-nav': true, active: page() === 'settings' }} onClick={() => setPage('settings')}><Icon name="settings" /><span>Settings</span></button><button class="profile-button" onClick={() => setPage('settings')}><Avatar actor={me()} name={profile().display_name || 'You'} photo={profilePhoto()} /><span><strong>{profile().display_name || 'Your space'}</strong><small>{me() || 'Sign in to get started'}</small></span><Icon name="chevron" size={16} /></button></div>
    </aside>
    <main>
      <header class="topbar"><span class="breadcrumb">Workspace <Icon name="chevron" size={13} /> <strong>{page() === 'calls' ? 'Calls' : page() === 'voicemail' ? 'Voicemail' : page() === 'devices' ? 'My devices' : 'Settings'}</strong></span><div class="topbar-right"><span class={`connection ${status()}`}><span />{status() === 'connected' ? 'Connected' : status() === 'reconnecting' ? 'Reconnecting' : status() === 'connecting' ? 'Connecting' : 'Offline'}</span><button class="icon-button" title="Refresh" aria-label="Refresh" disabled={!session() || busy()} onClick={() => void run(refresh)}><Icon name="refresh" size={17} /></button></div></header>
      <Show when={error()}><div class="alert error" role="alert"><span>{error()}</span><button aria-label="Dismiss error" onClick={() => setError('')}><Icon name="close" size={16} /></button></div></Show>
      <Show when={notice()}><div class="alert success" role="status"><Icon name="check" size={17} /><span>{notice()}</span><button aria-label="Dismiss notification" onClick={() => setNotice('')}><Icon name="close" size={16} /></button></div></Show>
      <div class="page-content">
        <Show when={session()} fallback={<div class="welcome-layout"><section class="welcome-story"><span class="eyebrow"><span /> THE HUMAN SIDE OF CONNECTION</span><h1>Different minds.<br />Same conversation<span>.</span></h1><p>A familiar way to talk to your people and your silicons. Just an ID, a ring, and a little less distance.</p><div class="orbit-scene" aria-hidden="true"><div class="orbit orbit-one" /><div class="orbit orbit-two" /><div class="orbit orbit-three" /><span class="orbit-person"><Icon name="user" size={28} /></span><span class="orbit-silicon"><Icon name="spark" size={28} /></span><div class="orbit-center"><Logo /></div><span class="orbit-label">GOOD THINGS START WITH HELLO</span></div><div class="welcome-features"><span><Icon name="mic" size={16} /> Natural conversations</span><span><Icon name="devices" size={16} /> Every device, one line</span></div></section><section class="login-card"><div class="login-icon"><Icon name="arrow" size={26} /></div><span class="eyebrow">YOUR LINE IS WAITING</span><h2>Make yourself at home.</h2><p>Connect with a short-lived token from IAM.</p><form onSubmit={e => { e.preventDefault(); void login(); }}><label for="iam-token">IAM access token</label><input id="iam-token" type="password" autocomplete="off" placeholder="Paste your IAM token" required value={token()} onInput={e => setToken(e.currentTarget.value)} /><span class="field-hint">Use your IAM CLI or consent screen to get a token for Ring.</span><button class="primary full" type="submit" disabled={busy() || !token().trim()}>{busy() ? 'Connecting…' : 'Connect to Ring'}<Icon name="arrow" size={18} /></button><button class="text-button advanced-button" type="button" onClick={() => setAdvanced(!advanced())}><Icon name="settings" size={14} />Connection settings<Icon name="chevron" size={12} /></button><Show when={advanced()}><div class="advanced"><label for="server">Server</label><input id="server" type="url" required value={server()} onInput={e => setServer(e.currentTarget.value)} /><label for="organization">Organization</label><input id="organization" value={org()} onInput={e => setOrg(e.currentTarget.value)} placeholder="Optional IAM organization" /><label for="realm">Environment</label><select id="realm" value={['production', 'test'].includes(realm()) ? realm() : 'custom'} onChange={e => setRealm(e.currentTarget.value === 'custom' ? '' : e.currentTarget.value)}><option value="production">Production</option><option value="test">Legacy test environment</option><option value="custom">Honeycomb test environment</option></select><Show when={!['production', 'test'].includes(realm())}><label for="environment-id">Environment UUID</label><input id="environment-id" required value={realm()} onInput={e => setRealm(e.currentTarget.value)} placeholder="Honeycomb environment UUID" autocomplete="off" /></Show><Show when={realm() !== 'production'}><label for="test-secret">Test app secret</label><input id="test-secret" type="password" value={testSecret()} onInput={e => setTestSecret(e.currentTarget.value)} autocomplete="off" /><small class="test-label">Isolated test environment. No production calls.</small></Show></div></Show></form><div class="login-footer"><span class="tiny-dot" /> YOUR IDENTITY, VERIFIED BY IAM</div></section></div>}>
          <Show when={page() === 'calls'}>
            <section class="page-heading"><div><span class="eyebrow">KEEP THE CONVERSATION GOING</span><h1>Your line is open<span>.</span></h1><p>A familiar voice is only a ring away.</p></div><button class="primary" disabled={status() !== 'connected'} onClick={() => openDial()}><Icon name="plus" size={18} />New call</button></section>
            <Show when={activeCall()}>{call => <button class="active-banner" onClick={() => selectCall(call())}><span class="live-pulse" /><div><strong>In a call with {otherName(call())}</strong><span>{callHere() ? 'Connected on this device' : 'Active on another device'} · {duration(Math.max(0, Math.floor((now() - new Date(call().answered_at || call().created_at).getTime()) / 1000)))}</span></div><span>Return to call <Icon name="arrow" size={17} /></span></button>}</Show>
            <div class="calls-layout"><section class="history-card"><div class="section-title"><h2>Recent calls <span class="count">{calls().length}</span></h2><span class="subtle">Your conversation history</span></div><div class="history-toolbar"><div class="segmented"><button classList={{ selected: filter() === 'all' }} onClick={() => setFilter('all')}>All calls</button><button classList={{ selected: filter() === 'missed' }} onClick={() => setFilter('missed')}>Missed</button></div><label class="search-box"><Icon name="search" size={16} /><input aria-label="Search calls" placeholder="Search calls" value={search()} onInput={e => setSearch(e.currentTarget.value)} /><span>⌕</span></label></div><Show when={visibleCalls().length} fallback={<Empty icon="phone" title={search() ? 'No matching conversations' : filter() === 'missed' ? 'All caught up' : 'Your first hello starts here'}>{search() ? 'Try a different name or ID.' : filter() === 'missed' ? 'No missed calls to return.' : 'Call a carbon or silicon by their ID. Your conversations will appear here.'}</Empty>}><div class="call-list"><div class="list-label">CONVERSATIONS <span>WHEN</span></div><For each={visibleCalls()}>{call => <button classList={{ 'call-row': true, selected: selected() === call.ringid }} onClick={() => selectCall(call)}><Avatar actor={other(call)} name={otherName(call)} photo={photos()[other(call)]} /><div class="call-person"><strong>{otherName(call)}<Show when={other(call)?.startsWith('si:')}><span class="silicon-tag">SILICON</span></Show></strong><span><Icon name={call.caller === me() ? 'arrow' : 'phone'} size={12} />{call.state === 'ended' ? (call.outcome || (call.caller === me() ? 'Outgoing' : 'Incoming')) : call.state === 'ringing' && incoming(call) ? 'Incoming call' : call.state} <span class="row-id">· {other(call)}</span></span></div><div class="call-time"><strong>{formatTime(call.created_at)}</strong><span>{formatDate(call.created_at)}</span></div><span class="row-call-icon"><Icon name="phone" size={17} /></span></button>}</For></div></Show><div class="history-footer"><span class="tiny-dot" /> Conversations, remembered.</div></section>
            <aside class="calls-aside"><Show when={selectedCall()} fallback={<><section class="dial-card"><div class="dial-card-top"><span class="eyebrow">A SHARED FREQUENCY</span><Icon name="spark" size={20} /></div><h2>Less typing.<br />More talking.</h2><p>People, assistants, and everyone in between. Bring them into the same conversation.</p><div class="sound-wave" aria-hidden="true"><For each={Array.from({ length: 39 }, (_, i) => 10 + Math.abs(Math.sin(i * 0.65)) * (20 + Math.sin(i * 0.17) * 28))}>{height => <span style={{ height: `${height}px` }} />}</For></div><button onClick={() => openDial()}>Start a conversation <Icon name="arrow" size={17} /></button></section><section class="contact-card"><h3>In your orbit</h3><p>A shortcut to familiar voices.</p><Show when={recentContacts().length} fallback={<div class="orbit-empty"><span>+</span><small>Your recent connections<br />will find a home here.</small></div>}><For each={recentContacts()}>{contact => <button class="contact-row" onClick={() => { setInviteMode(false); setTarget(contact.actor); setDialOpen(true); }}><Avatar actor={contact.actor} name={contact.name} photo={photos()[contact.actor]} /><span><strong>{contact.name}</strong><small>{contact.actor}</small></span><Icon name="phone" size={15} /></button>}</For></Show></section></>}>{call => <section class={`call-detail ${['active', 'connecting', 'ringing'].includes(call().state) ? 'live-detail' : ''}`}><div class="detail-top"><span class="eyebrow">{incoming(call()) ? 'INCOMING CALL' : call().state.toUpperCase()}</span><button class="icon-button" aria-label="Close call details" onClick={() => setSelected('')}><Icon name="close" size={17} /></button></div><Avatar actor={other(call())} name={otherName(call())} photo={photos()[other(call())]} large /><h2>{otherName(call())}</h2><span class="detail-id">{other(call())}</span><Show when={call().participants?.length > 2}><p class="roster">With {call().participants.filter(p => !p.left_at).map(p => p.display_name || displayActor(p.actor)).join(', ')}</p></Show><Show when={call().state === 'active' && !incoming(call())}><span class="call-duration"><span class="tiny-dot" />{duration(Math.max(0, Math.floor((now() - new Date(call().answered_at || call().created_at).getTime()) / 1000)))}</span><div class="live-wave" aria-label={audioReady() ? 'Microphone connected' : 'Microphone disconnected'}><For each={Array.from({ length: 23 }, (_, i) => i)}>{i => <span style={{ height: `${7 + (audioReady() && !muted() ? audioLevel() * 70 : 0) * Math.abs(Math.sin(i * 0.7))}px` }} />}</For></div><Show when={callHere()} fallback={<button class="light-button full" onClick={() => void moveHere()} disabled={busy()}>Move call to this device</button>}><Show when={!audioReady()}><button class="light-button full" onClick={() => void run(() => startAudio(call()))}>Connect microphone</button></Show><div class="call-controls"><button classList={{ 'round-control': true, muted: muted() }} disabled={!audioReady()} aria-label={muted() ? 'Unmute microphone' : 'Mute microphone'} onClick={() => void run(async () => { await audio.mute(api, !muted()); setMuted(audio.muted); })}><Icon name={muted() ? 'muted' : 'mic'} /></button><button class="round-control" aria-label="Invite someone" onClick={() => openDial(true)}><Icon name="plus" /></button><button class="round-control hangup" aria-label="End call" onClick={() => void callAction('calls.cut')}><Icon name="end" /></button></div><span class="microphone-label">{audioReady() ? muted() ? 'Microphone muted' : 'You’re connected' : 'Microphone not connected'}</span></Show></Show>
              <Show when={call().state === 'ringing' || incoming(call())}><p class="ringing-label">{incoming(call()) ? 'Someone is reaching out.' : 'Waiting for an answer…'}</p><Show when={incoming(call())} fallback={<button class="danger full" onClick={() => void callAction('calls.cut')} disabled={busy()}>Cancel call</button>}><div class="desktop-answer"><button class="primary full" onClick={() => void callAction('calls.accept', { device_id: session()?.device_id })} disabled={busy()}><Icon name="phone" size={18} />Accept call</button></div><label class="slide-answer"><span>Slide to answer →</span><input type="range" min="0" max="100" value="0" aria-label="Slide right to answer call" onChange={e => { if (+e.currentTarget.value > 85) void callAction('calls.accept', { device_id: session()?.device_id }); e.currentTarget.value = '0'; }} /></label><Show when={declineOpen()}><div class="decline-reasons"><For each={['I’m in a meeting', 'I’ll call you back', 'Can’t talk right now']}>{reason => <button onClick={() => void callAction('calls.decline', { reason })}>{reason}</button>}</For><input aria-label="Reason for declining" placeholder="Or leave a reason…" value={declineReason()} onInput={e => setDeclineReason(e.currentTarget.value)} /></div></Show><div class="answer-secondary"><button onClick={() => declineOpen() ? void callAction('calls.decline', declineReason() ? { reason: declineReason() } : { give_no_reason: true }) : setDeclineOpen(true)}>Decline{declineOpen() ? declineReason() ? ' with reason' : ' without reason' : ''}</button><button onClick={() => void callAction('calls.silence')}>Silence</button></div></Show></Show>
              <Show when={call().state === 'voicemail' && call().caller === me()}><p class="ringing-label">They’re unavailable. Leave a little hello.</p><Show when={vmDraft()} fallback={<button class="primary full" disabled={busy()} onClick={() => void beginVoicemail()}><Icon name="mic" size={18} />Record voicemail</button>}><span class="recording-label"><span /> Recording {duration(Math.max(0, Math.floor((now() - vmStarted()) / 1000)))}</span><button class="primary full" disabled={busy()} onClick={() => void finishVoicemail(true)}>Finish & send</button><button class="text-button" disabled={busy()} onClick={() => void finishVoicemail(false)}>Discard recording</button></Show></Show>
              <Show when={call().state === 'ended'}><p class="ended-label">{call().outcome || 'Call ended'} · {formatDate(call().created_at)}</p><button class="primary full" onClick={() => { setInviteMode(false); setTarget(other(call())); setDialOpen(true); }}><Icon name="phone" size={17} />Call again</button><button class="text-button download-button" disabled={busy()} onClick={() => void saveRecording(call())}><Icon name="download" size={15} />Download recording</button></Show>
              <div class="transcript"><h3>Conversation <span>{call().state === 'active' ? 'LIVE' : 'TRANSCRIPT'}</span></h3><Show when={transcript().length} fallback={<p class="transcript-empty">{call().state === 'active' ? 'Words will appear here as the conversation unfolds.' : 'No transcript entries yet.'}</p>}><div class="transcript-lines" aria-live="polite"><For each={transcript()}>{entry => <div classList={{ 'transcript-line': true, lifecycle: !entry.text && !entry.content && !entry.data?.text }}><span>{displayActor(entry.actor || (entry.speaker_ids || entry.data?.speaker_ids)?.join(' & ') || entry.type || entry.kind)}</span><p>{entry.text || entry.content || entry.data?.text || (entry.type || entry.kind || 'Call update').replaceAll('.', ' ')}</p></div>}</For></div></Show></div>
            </section>}</Show></aside></div>
          </Show>
          <Show when={page() === 'voicemail'}><section class="page-heading"><div><span class="eyebrow">SOMETHING TO COME BACK TO</span><h1>A hello, on hold<span>.</span></h1><p>Messages that keep the conversation open.</p></div><span class="count-pill">{unread()} unread</span></section><section class="panel"><div class="section-title"><h2>Your voicemail</h2><span class="subtle">{voicemails().length} messages</span></div><Show when={voicemails().length} fallback={<Empty icon="voicemail" title="A little quiet, for now">When someone leaves a message, you’ll find their voice and transcript here.</Empty>}><For each={voicemails()}>{vm => <div class="vm-row"><button class="vm-main" onClick={() => void openVoicemail(vm)}><Avatar actor={vm.sender || vm.from_actor} /><span><strong>{displayActor(vm.sender || vm.from_actor)}<Show when={!vm.read}><span class="unread-dot" /></Show></strong><p>{vm.transcript || 'Audio message · open to listen'}</p><small>{vm.created_at ? `${formatDate(vm.created_at)} · ${formatTime(vm.created_at)}` : 'Voicemail'}</small></span><span class="play-icon"><Icon name="play" size={15} /></span></button><button class="text-button" onClick={() => void run(async () => { await api.request('voicemail.mark', { voicemail_id: vm.voicemail_id, read: !vm.read }); await refresh(); })}>{vm.read ? 'Mark unread' : 'Mark read'}</button></div>}</For></Show></section></Show>
          <Show when={page() === 'devices'}><section class="page-heading"><div><span class="eyebrow">WHEREVER YOU FEEL AT HOME</span><h1>One you. Every device<span>.</span></h1><p>Keep your line with you. Pick up where you left off.</p></div></section><section class="panel"><div class="section-title"><h2>Connected devices</h2><span class="subtle">{devices().length} registered</span></div><For each={devices()}>{device => <div class="device-row"><span class="device-icon"><Icon name="devices" size={28} /></span><div class="device-info"><strong>{device.name || 'Ring device'}<Show when={device.device_id === session()?.device_id}><span class="device-here">This device</span></Show></strong><span>{device.device_id}</span><small>{device.online || device.available ? 'Online' : 'Registered'}{(device.active_ringid || device.active_call) ? ' · On a call' : ''}</small></div><label class="toggle-row"><span>Ring here</span><input type="checkbox" checked={device.ring_enabled !== false} onChange={e => void run(async () => { await api.request('devices.update', { device_id: device.device_id, ring_enabled: e.currentTarget.checked }); await refresh(); })} /><span class="toggle" /></label></div>}</For><div class="device-footnote"><Icon name="bell" size={20} /><div><strong>Stay within earshot.</strong><p>Keep Ring open to receive calls. Mobile background calling requires platform push credentials and native call integration.</p></div><button class="secondary" onClick={() => void notifications()}>Enable notifications</button></div></section></Show>
          <Show when={page() === 'settings'}><section class="page-heading"><div><span class="eyebrow">MAKE IT YOURS</span><h1>Your kind of connection<span>.</span></h1><p>A few small things that make Ring feel like you.</p></div></section><form class="settings-panel" onSubmit={e => { e.preventDefault(); void saveSettings(); }}><section><div class="settings-section-label"><Icon name="user" /><div><h2>Your profile</h2><p>How you appear when you call.</p></div></div><div class="profile-edit"><Avatar actor={me()} name={name()} photo={profilePhoto()} large /><label class="secondary upload-label">Change photo<input type="file" accept="image/png,image/jpeg,image/webp" onChange={e => void photoUpload(e.currentTarget.files?.[0])} /></label></div><label for="display-name">Display name</label><input id="display-name" required maxlength="100" value={name()} onInput={e => { settingsDirty = true; setName(e.currentTarget.value); }} /><span class="field-hint">{me()} · {session()?.org_id}</span></section><section><div class="settings-section-label"><Icon name="voicemail" /><div><h2>Voicemail</h2><p>Leave the door open when you’re away.</p></div></div><label class="toggle-row settings-toggle"><span><strong>Allow voicemail</strong><small>Let callers leave an audio message.</small></span><input type="checkbox" checked={voicemailEnabled()} onChange={e => { settingsDirty = true; setVoicemailEnabled(e.currentTarget.checked); }} /><span class="toggle" /></label><label for="greeting">Declined call greeting</label><textarea id="greeting" rows="3" placeholder="Leave empty to use the default greeting." value={greeting()} onInput={e => { settingsDirty = true; setGreeting(e.currentTarget.value); }} /></section><section><div class="settings-section-label"><Icon name="settings" /><div><h2>Preferences</h2><p>Little things, your way.</p></div></div><label class="toggle-row settings-toggle"><span><strong>Help improve Ring</strong><small>Share diagnostic events. Conversation content is excluded.</small></span><input type="checkbox" checked={telemetry()} onChange={e => { settingsDirty = true; setTelemetry(e.currentTarget.checked); }} /><span class="toggle" /></label><div class="settings-connection"><span class="tiny-dot" />{server()}<Show when={realm() !== 'production'}><span class="test-label">TEST{realm() !== 'test' ? ` · ${realm()}` : ''}</span></Show></div></section><div class="settings-actions"><button class="text-button" type="button" onClick={() => void logout()}><Icon name="logout" size={17} />Sign out</button><button class="primary" type="submit" disabled={busy()}>{busy() ? 'Saving…' : 'Save changes'}<Icon name="check" size={17} /></button></div></form></Show>
        </Show>
      </div><footer class="page-footer"><span>RING · A TEAM OF SILICONS APP</span><span>Made for the conversation.</span></footer>
    </main>
    <Show when={dialOpen()}><div class="modal-backdrop" onClick={e => { if (e.target === e.currentTarget) setDialOpen(false); }}><section class="modal" role="dialog" aria-modal="true" aria-labelledby="dial-title"><button class="modal-close icon-button" aria-label="Close" onClick={() => setDialOpen(false)}><Icon name="close" /></button><span class="modal-symbol"><Icon name={inviteMode() ? 'plus' : 'phone'} size={26} /></span><span class="eyebrow">{inviteMode() ? 'ROOM FOR ONE MORE' : 'A LITTLE LESS DISTANCE'}</span><h2 id="dial-title">{inviteMode() ? 'Bring someone in.' : 'Who’s on your mind?'}</h2><p>{inviteMode() ? 'Invite a carbon or silicon into this conversation.' : 'Enter a Carbon or Silicon ID to start a call.'}</p><Show when={error()}><p class="modal-error" role="alert">{error()}</p></Show><form onSubmit={e => { e.preventDefault(); void dial(); }}><label for="call-target">Carbon or Silicon ID</label><input id="call-target" autofocus placeholder="c:alex or si:assistant" value={target()} onInput={e => setTarget(e.currentTarget.value)} /><span class="field-hint">Call any registered carbon or silicon by their global ID, across organizations.</span><button class="primary full" type="submit" disabled={busy() || !target().trim()}><Icon name="phone" size={18} />{busy() ? 'Connecting…' : inviteMode() ? 'Send invitation' : 'Start call'}</button></form><Show when={recentContacts().length}><div class="dial-recents"><span class="eyebrow">RECENT CONNECTIONS</span><For each={recentContacts().slice(0, 3)}>{contact => <button class="contact-row" onClick={() => setTarget(contact.actor)}><Avatar actor={contact.actor} name={contact.name} photo={photos()[contact.actor]} /><span><strong>{contact.name}</strong><small>{contact.actor}</small></span><Icon name="arrow" size={16} /></button>}</For></div></Show></section></div></Show>
    <Show when={openedVm()}><div class="modal-backdrop" onClick={e => { if (e.target === e.currentTarget) setOpenedVm(null); }}><section class="modal" role="dialog" aria-modal="true" aria-labelledby="vm-title"><button class="modal-close icon-button" aria-label="Close voicemail" onClick={() => setOpenedVm(null)}><Icon name="close" /></button><span class="eyebrow">A MESSAGE FOR YOU</span><h2 id="vm-title">{displayActor(openedVm().sender || openedVm().from_actor)}</h2><Show when={vmAudio()}><audio controls src={vmAudio()} /></Show><p class="voicemail-transcript">{openedVm().transcript || `Transcript ${openedVm().transcription_status || 'not available yet'}.`}</p><button class="secondary" onClick={() => void run(async () => { await api.request('voicemail.mark', { voicemail_id: openedVm().voicemail_id, read: true }); setOpenedVm(null); await refresh(); })}><Icon name="check" size={17} />Mark as read</button></section></div></Show>
  </div>;
}
