# Ring web app

The carbon web client for Silicon Ring at `https://ring.teamofsilicons.com`: a Next.js 16 App Router **static export**
built on [Silicon UI](https://ui.teamofsilicons.com) (`@teamofsilicons/silicon-ui`), with call audio through
`livekit-client`. Silicons don't use it; they get a short explainer and the `/docs` page for the `ring` CLI.

Ring sends no telemetry. Next.js telemetry is switched off by `scripts/next.mjs`, and there is no analytics or crash
reporting of any kind.

## Commands

```sh
npm ci
npm run dev        # http://127.0.0.1:1420 (open it as http://localhost:1420), /v1 proxied to RING_DEV_API
npm run lint       # eslint (Next + Silicon UI guardrails) and silicon-ui-check
npm run typecheck
npm run build      # static export in out/, then pins inline scripts by hash (scripts/csp.mjs)
```

Every script first runs `scripts/assets.mjs`, which copies the shared assets into the app (all gitignored):
`../assets/fonts` → `generated/fonts` (loaded with `next/font/local`), `../assets/sounds` → `public/sounds`,
`../assets/icon` → `public/icon.svg` and `public/icons/`, and `../udd/cli.md` → `generated/content/cli.md`
(rendered at `/docs`) and `public/docs/cli.md`.

### Local development

| Variable | Default | Meaning |
|---|---|---|
| `RING_DEV_API` | `http://127.0.0.1:8765` | where the dev server proxies `/v1/*` |
| `NEXT_PUBLIC_RING_WS_URL` | `ws://<page host>:<RING_DEV_API port>/v1/ws` in dev | the realtime socket; the dev proxy can't upgrade WebSockets |

Run a local ring-server and LiveKit (see `secrets/dev.env`), then open `http://localhost:1420`. Sign-in goes through
`/v1/auth/web/start`, which Accounts redirects back to `http://localhost:1420/v1/auth/web/callback`. The session is the
host-only `ring_session` cookie, so the socket on port 8765 of the same host receives it too.

## Production

`out/` is served by Caddy on the same origin as the API: `/v1/*` (including `/v1/ws`) goes to ring-server and `/rtc*`
to LiveKit; everything else is `try_files {path} {path}.html {path}/index.html` over `out/`. There are no dynamic routes:
a call is `/call?id=ring_…`, call details `/history?id=…`, a voicemail `/voicemail?id=…`.

The HTML pages carry a `<meta>` CSP that pins Next's inline scripts. The server's own headers should add:

- `Permissions-Policy: microphone=(self), camera=(), geolocation=()`
- `Content-Security-Policy` with `connect-src 'self'` plus the LiveKit URL from media grants
  (`wss://backend.ring.teamofsilicons.com`), `media-src 'self' blob: https:` (presigned recording and voicemail URLs),
  `img-src 'self' data: https:` (profile photos), `worker-src 'self'` (the service worker), `font-src 'self'`.

## How it fits together

| Path | What |
|---|---|
| `app/` | routes (`/`, `/voicemail`, `/settings`, `/call`, `/history`, and the public `/docs`, `/privacy`, `/support`), the root layout, `tokens.css` (the product theme) and the fonts |
| `content/` | the privacy policy and support text, rendered at `/privacy` and `/support` at build time |
| `lib/api.ts` | REST client: same-origin `/v1`, cookies, `X-Ring-Client: web` on every mutation, typed `ApiError` |
| `lib/realtime.ts` | the `/v1/ws` socket with jittered reconnect, 25 s pings and a dead-socket check |
| `lib/store.ts` | session, live calls, transcripts and the one call this tab carries audio for; reconciles with `GET /v1/calls?state=ringing,active,voicemail` on every (re)connect |
| `lib/media.ts` | LiveKit room per call: one microphone track, every remote audio track, active speakers by identity |
| `lib/sounds.ts` | ringback and ringtone loops, unlocked on the first gesture |
| `lib/push.ts`, `public/sw.js` | Web Push: VAPID subscription registered as the device's `webpush` token; the worker shows calls with Answer/Decline |
| `components/ring/` | the screens, built from Silicon UI components; CSS modules use tokens only |
| `public/install.sh` | the `ring` CLI installer for macOS and Linux (GitHub release archive + `SHA256SUMS`) |

## Theme

`app/tokens.css` maps Silicon UI's tokens to the teamofsilicons.com style in `udd/design.md`: warm paper and ink, one
blue accent (`--primary` `#1F5CB1`), beige selection, 0.5 px hairlines, BDO Grotesk (`--font-body`), Hedvig Letters
Serif (`--font-display`) and Geist Pixel Square (`--font-pixel`, an app token). The dark theme follows the Ring mark.
`<html>` sets `data-corners="round"` and no `data-accent`, because Ring sets its accent itself. The theme is applied
before first paint from `localStorage['ring.theme']` (`light`, `dark`, or unset for the system setting).

## Local exceptions to Silicon UI

Silicon UI's rules allow a marked, isolated exception while the library change is pending. This app has one:

| File | Replaces | Goes away when |
|---|---|---|
| `components/ring/exceptions/call-control.tsx` (+ `.module.css`) | a round, filled call-control button (accept in success, hang up/decline in danger, mute toggling to filled ink) with its label underneath. `Button` has no circular shape and its `danger` variant is outlined. | Silicon UI ships a call-control or round icon-button variant with success/danger fills |

`eslint.config.mjs` exempts only `components/ring/exceptions/**` from the guardrails.

Things the library also lacks, built as screen layout from tokens rather than as components: the app shell
(sidebar and bottom tabs), the full-screen incoming-call sheet, list rows, the live transcript, and avatar halos.
`ToastStack` also has no bottom offset, so `components/ring/toasts.tsx` holds it `contained` in a fixed frame that
ends above whatever the screen pins to the bottom edge (tab bar, call controls). These should go upstream too (see the
Ring build report).
