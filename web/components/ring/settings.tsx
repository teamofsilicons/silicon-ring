'use client';

import { useEffect, useState } from 'react';
import { ExternalLink, LogOut, Monitor, Smartphone, Terminal } from 'lucide-react';
import { Alert } from '@teamofsilicons/silicon-ui/alert';
import { Badge } from '@teamofsilicons/silicon-ui/badge';
import { Button } from '@teamofsilicons/silicon-ui/button';
import { ButtonLink } from '@teamofsilicons/silicon-ui/button-link';
import { ConfirmMorph } from '@teamofsilicons/silicon-ui/confirm-morph';
import { Input } from '@teamofsilicons/silicon-ui/input';
import SegmentedControl from '@teamofsilicons/silicon-ui/segmented-control';
import { Select } from '@teamofsilicons/silicon-ui/select';
import { Skeleton } from '@teamofsilicons/silicon-ui/skeleton';
import { Switch } from '@teamofsilicons/silicon-ui/switch';
import { Textarea } from '@teamofsilicons/silicon-ui/textarea';
import { useToastStack } from '@teamofsilicons/silicon-ui/toast-stack';
import { api, errorMessage } from '@/lib/api';
import { codePoints, platformName, when } from '@/lib/format';
import { APP_VERSION } from '@/lib/store';
import { readAppearance, saveAppearance, type Appearance } from '@/lib/theme';
import type { Device, Greetings, Settings, Voice } from '@/lib/types';
import { GetTheApps } from './get-apps';
import { PersonAvatar, Pixel } from './identity';
import { PageHeader } from './page';
import { useRing, useRingStore } from './ring-context';
import lists from './lists.module.css';
import styles from './settings.module.css';

export function SettingsScreen() {
  return (
    <>
      <PageHeader title="Settings" />
      <div className={styles.sections}>
        <AccountSection />
        <ThisDeviceSection />
        <DevicesSection />
        <AppsSection />
        <VoicemailSection />
        <AppearanceSection />
        <AboutSection />
        <SignOutSection />
      </div>
    </>
  );
}

function Group({ title, children, description }: { title: string; children: React.ReactNode; description?: string }) {
  return (
    <section className={styles.group} aria-label={title}>
      <Pixel as="h2">{title}</Pixel>
      {description ? <p className={styles.description}>{description}</p> : null}
      <div className={styles.card}>{children}</div>
    </section>
  );
}

function AccountSection() {
  const me = useRing(state => state.me);
  const accountsUrl = useRing(state => state.meta?.accounts_url ?? 'https://accounts.teamofsilicons.com');
  if (!me) return null;
  return (
    <Group title="Account">
      <div className={styles.account}>
        <PersonAvatar account={me.account} size="lg" />
        <div className={styles.accountText}>
          <span className={styles.accountName}>{me.account.display_name || me.account.id}</span>
          <Pixel keepCase>{me.account.id}</Pixel>
        </div>
      </div>
      <div className={styles.rowEnd}>
        <ButtonLink href={accountsUrl} target="_blank" rel="noreferrer" variant="secondary" size="sm">
          Edit profile on Silicon Accounts
          <ExternalLink size={14} strokeWidth={1.75} aria-hidden="true" />
        </ButtonLink>
      </div>
    </Group>
  );
}

function DeviceNameForm({ current }: { current: string }) {
  const store = useRingStore();
  const { toast } = useToastStack();
  const [name, setName] = useState(current);
  const [saving, setSaving] = useState(false);
  const next = name.trim();
  const tooLong = codePoints(next) > 80;

  async function save() {
    if (!next || next === current || tooLong) return;
    setSaving(true);
    try {
      await store.updateThisDevice({ name: next });
      toast({ type: 'success', title: 'Device renamed' });
    } catch (error) {
      toast({ type: 'error', title: 'Couldn’t rename this device', description: errorMessage(error) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <form
      className={styles.inline}
      onSubmit={event => {
        event.preventDefault();
        void save();
      }}
    >
      <Input
        label="Device name"
        value={name}
        maxLength={120}
        onChange={event => setName(event.target.value)}
        error={tooLong ? 'Keep it to 80 characters.' : undefined}
        description="Shown on your other devices, such as “Active on Chrome on macOS”."
      />
      <Button type="submit" size="sm" variant="secondary" loading={saving} disabled={!next || next === current || tooLong}>Save name</Button>
    </form>
  );
}

function ThisDeviceSection() {
  const store = useRingStore();
  const device = useRing(state => state.me?.device);
  const push = useRing(state => state.push);
  const pushActive = useRing(state => state.pushActive);
  const { toast } = useToastStack();
  const [enabling, setEnabling] = useState(false);

  if (!device) return null;

  return (
    <Group title="This device">
      <DeviceNameForm key={device.name ?? ''} current={device.name ?? ''} />
      <div className={styles.switchRow}>
        <Switch
          label="Ring on this device"
          checked={device.ring_enabled}
          onCheckedChange={async checked => {
            try {
              await store.updateThisDevice({ ring_enabled: checked });
            } catch (error) {
              toast({ type: 'error', title: 'Couldn’t change ringing', description: errorMessage(error) });
            }
          }}
        />
      </div>
      <div className={styles.notifications}>
        <div className={styles.notificationsText}>
          <span className={styles.itemTitle}>Calls while Ring is closed</span>
          <span className={styles.description}>
            {push === 'unsupported' ? 'This browser can’t receive push notifications. Keep the Ring tab open to get calls.'
              : push === 'denied' ? 'Notifications are blocked. Allow them for this site in your browser’s settings.'
                : push === 'granted' && pushActive ? 'Incoming calls show a notification with Answer and Decline, even with no Ring tab open.'
                  : 'Allow notifications so calls reach you when no Ring tab is open.'}
          </span>
        </div>
        {push === 'default' || (push === 'granted' && !pushActive) ? (
          <Button
            size="sm"
            variant="primary"
            loading={enabling}
            onClick={async () => {
              setEnabling(true);
              try {
                if (push === 'granted') await store.syncDevice();
                else await store.enableNotifications();
              } catch (error) {
                toast({ type: 'error', title: 'Notifications didn’t turn on', description: errorMessage(error) });
              } finally {
                setEnabling(false);
              }
            }}
          >
            Turn on
          </Button>
        ) : push === 'granted' ? <Badge size="sm" tone="success">ON</Badge> : null}
      </div>
    </Group>
  );
}

function DeviceIcon({ device }: { device: Device }) {
  const props = { size: 18, strokeWidth: 1.6, 'aria-hidden': true } as const;
  if (device.platform === 'cli') return <Terminal {...props} />;
  if (device.platform === 'ios' || device.platform === 'android' || device.platform === 'ipados') return <Smartphone {...props} />;
  return <Monitor {...props} />;
}

function DevicesSection() {
  const revision = useRing(state => state.revision.devices);
  const { toast } = useToastStack();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.devices().then(
      result => { if (!cancelled) { setDevices(result.items); setError(null); } },
      failure => { if (!cancelled) setError(errorMessage(failure)); },
    );
    return () => { cancelled = true; };
  }, [revision]);

  return (
    <Group title="Devices" description="Every device signed in to Ring rings when you get a call.">
      {error ? <Alert tone="danger" title="Devices didn’t load">{error}</Alert> : null}
      {!devices && !error ? <Skeleton lines={3} label="Loading devices" /> : null}
      {devices ? (
        <ul className={styles.devices}>
          {devices.map(device => (
            <li key={device.device_id} className={styles.device}>
              <span className={styles.deviceIcon}><DeviceIcon device={device} /></span>
              <span className={lists.rowText}>
                <span className={lists.rowTitle}>
                  <span className={lists.ellipsis}>{device.name || platformName(device.platform)}</span>
                  {device.current ? <Badge size="sm" tone="info">THIS BROWSER</Badge> : null}
                </span>
                <span className={lists.metaText}>
                  {[platformName(device.platform), device.online ? 'Online' : device.last_seen_at ? `Seen ${when(device.last_seen_at)}` : null, device.ring_enabled ? null : 'Ringing off']
                    .filter(Boolean).join(' · ')}
                </span>
              </span>
              {device.current ? <span /> : (
                <ConfirmMorph
                  label="Revoke"
                  prompt="Sign it out?"
                  confirmLabel="Revoke"
                  pendingLabel="Revoking"
                  doneLabel="Revoked"
                  tone="danger"
                  onConfirm={async () => {
                    try {
                      await api.revokeDevice(device.device_id);
                      setDevices(current => current?.filter(item => item.device_id !== device.device_id) ?? null);
                    } catch (failure) {
                      toast({ type: 'error', title: 'Couldn’t revoke that device', description: errorMessage(failure) });
                      throw failure;
                    }
                  }}
                />
              )}
            </li>
          ))}
        </ul>
      ) : null}
    </Group>
  );
}

// A stable empty list: a selector must not return a new array on every read.
const NO_VOICES: Voice[] = [];

const GREETINGS: { key: keyof Greetings; label: string; description: string }[] = [
  { key: 'missed', label: 'When you miss a call', description: 'Played when nobody answers within a minute.' },
  { key: 'declined', label: 'When you decline', description: 'Played after you decline a call.' },
  { key: 'busy', label: 'When you’re on another call', description: 'Played when you’re already talking to someone.' },
];

function AppsSection() {
  return (
    <Group title="Apps" description="Ring rings on every device you sign in to, even with its app closed. Install it where you take calls.">
      <GetTheApps align="start" highlight />
    </Group>
  );
}

function VoicemailSection() {
  const { toast } = useToastStack();
  const voices = useRing(state => state.meta?.voices ?? NO_VOICES);
  const limit = useRing(state => state.meta?.limits.greeting ?? 300);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.settings().then(setSettings, failure => setError(errorMessage(failure)));
  }, []);

  async function patch(update: Parameters<typeof api.patchSettings>[0], success: string) {
    try {
      setSettings(await api.patchSettings(update));
      toast({ type: 'success', title: success });
    } catch (failure) {
      toast({ type: 'error', title: 'Couldn’t save that', description: errorMessage(failure) });
      throw failure;
    }
  }

  return (
    <Group title="Voicemail" description="When you can’t answer, callers hear your greeting, then a beep, and can leave a message.">
      {error ? <Alert tone="danger" title="Voicemail settings didn’t load">{error}</Alert> : null}
      {!settings && !error ? <Skeleton lines={3} label="Loading voicemail settings" /> : null}
      {settings ? (
        <>
          <div className={styles.switchRow}>
            <Switch
              label="Take voicemail"
              checked={settings.voicemail.enabled}
              onCheckedChange={checked => void patch({ voicemail: { enabled: checked } }, checked ? 'Voicemail is on' : 'Voicemail is off').catch(() => undefined)}
            />
          </div>
          {voices.length ? (
            <Select
              label="Greeting voice"
              description="Your greetings are read aloud in this Natural Voice."
              value={settings.voice}
              onValueChange={voice => void patch({ voice }, 'Greeting voice saved').catch(() => undefined)}
              options={voices.map(voice => ({ value: voice.id, label: `${voice.name} · ${voice.description}` }))}
            />
          ) : null}
          {GREETINGS.map(greeting => (
            <GreetingEditor
              key={`${greeting.key}:${settings.voicemail.greetings[greeting.key] ?? ''}`}
              label={greeting.label}
              description={greeting.description}
              value={settings.voicemail.greetings[greeting.key]}
              limit={limit}
              disabled={!settings.voicemail.enabled}
              onSave={text => patch({ voicemail: { greetings: { [greeting.key]: text } } }, text === null ? 'Greeting reset' : 'Greeting saved')}
            />
          ))}
        </>
      ) : null}
    </Group>
  );
}

function GreetingEditor({ label, description, value, limit, disabled, onSave }: {
  label: string;
  description: string;
  value: string | null;
  limit: number;
  disabled: boolean;
  onSave: (text: string | null) => Promise<void>;
}) {
  const [text, setText] = useState(value ?? '');
  const [busy, setBusy] = useState<'save' | 'reset' | null>(null);
  const length = codePoints(text.trim());
  const changed = text.trim() !== (value ?? '');

  async function run(kind: 'save' | 'reset', next: string | null) {
    setBusy(kind);
    try {
      await onSave(next);
    } catch {
      // The toast already explains it.
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className={styles.greeting}>
      <Textarea
        label={label}
        description={value === null ? `${description} Ring’s default greeting is used.` : description}
        placeholder="Ring’s default greeting"
        value={text}
        rows={2}
        disabled={disabled}
        onChange={event => setText(event.target.value)}
        error={length > limit ? `Keep it to ${limit} characters (${length} now).` : undefined}
      />
      <div className={styles.greetingActions}>
        <Pixel>{length}/{limit}</Pixel>
        <Button size="sm" variant="ghost" disabled={disabled || value === null || Boolean(busy)} loading={busy === 'reset'} onClick={() => void run('reset', null)}>Reset</Button>
        <Button size="sm" variant="secondary" disabled={disabled || !changed || !text.trim() || length > limit || Boolean(busy)} loading={busy === 'save'} onClick={() => void run('save', text.trim())}>Save</Button>
      </div>
    </div>
  );
}

function AppearanceSection() {
  const [appearance, setAppearance] = useState<Appearance>(readAppearance);
  return (
    <Group title="Appearance">
      <SegmentedControl
        label="Appearance"
        value={appearance}
        onValueChange={value => {
          const next = value as Appearance;
          setAppearance(next);
          saveAppearance(next);
        }}
        options={[
          { value: 'system', label: 'System' },
          { value: 'light', label: 'Light' },
          { value: 'dark', label: 'Dark' },
        ]}
      />
    </Group>
  );
}

function AboutSection() {
  const serverVersion = useRing(state => state.meta?.version);
  return (
    <Group title="About">
      <dl className={styles.about}>
        <div><dt>Web app</dt><dd>{APP_VERSION}</dd></div>
        {serverVersion ? <div><dt>Server</dt><dd>{serverVersion}</dd></div> : null}
        <div><dt>Privacy</dt><dd>Ring sends no telemetry.</dd></div>
      </dl>
      <div className={styles.rowEnd}>
        <ButtonLink href="https://github.com/teamofsilicons/silicon-ring" target="_blank" rel="noreferrer" variant="ghost" size="sm">
          Source
          <ExternalLink size={14} strokeWidth={1.75} aria-hidden="true" />
        </ButtonLink>
        <ButtonLink href="/docs" variant="ghost" size="sm">ring CLI for silicons</ButtonLink>
      </div>
    </Group>
  );
}

function SignOutSection() {
  const store = useRingStore();
  const [busy, setBusy] = useState(false);
  return (
    <div className={styles.signOut}>
      <Button
        variant="danger"
        loading={busy}
        onClick={async () => {
          setBusy(true);
          await store.signOut();
        }}
      >
        <LogOut size={16} strokeWidth={1.75} aria-hidden="true" />
        Sign out of this browser
      </Button>
    </div>
  );
}
