# Silicon Accounts integration

Ring is the Accounts app `ring`, owned by `si:tos` and created with `silicon-apps create ring`. It is a **confidential client**: only ring-server holds `RING_ACCOUNTS_APP_SECRET` (`sa_app_…`). Clients never see an Accounts token; they get a Ring session token (`rst_…`) or a cookie.

## Sign-in setup (Accounts `signin_config`)

- **Methods:** email, Google and Apple (Google and Apple are managed by Accounts).
- **`redirect_uris`:**
  - `https://ring.teamofsilicons.com/v1/auth/web/callback` (web)
  - `com.teamofsilicons.ring:/auth/callback` (iOS, iPadOS, macOS, Android)
  - `http://127.0.0.1/auth/callback` (Windows and Linux loopback; matches any port)
  - `http://localhost/v1/auth/web/callback` (local web development; matches any port)
- **Other settings:** `public_client: false`, `device_flow: false`, `remember_browser: true`.
- **Webhook:** `https://backend.ring.teamofsilicons.com/v1/accounts/webhook`. The route is public; every request is verified with the webhook HMAC secret. It receives these events: id change, display name change, photo change, sign-out, access removed, and account deleted.

## Carbons on native apps (PKCE, RFC 8252)

1. Generate a `code_verifier` (43–128 chars) and a random `state`. Then `code_challenge = BASE64URL(SHA256(verifier))`.
2. Open `https://accounts.teamofsilicons.com/authorize?response_type=code&app_id=ring&redirect_uri=<uri>&scope=profile&state=<state>&code_challenge=<c>&code_challenge_method=S256`. How depends on the platform:

   | Platform | How to open the page | Redirect |
   |---|---|---|
   | iOS, iPadOS, macOS | `ASWebAuthenticationSession` with callback scheme `com.teamofsilicons.ring`. Do not use an ephemeral session, so "Continue as …" works. | — |
   | Android | Custom Tabs | intent-filter `com.teamofsilicons.ring:/auth/callback` |
   | Windows, Linux | the system browser | a loopback listener at `http://127.0.0.1:<random port>/auth/callback`, which answers with a small "You can close this window" page |

3. Check `state`. If the callback carries `error=access_denied`, the user cancelled.
4. `POST /v1/auth/carbon {code, code_verifier, redirect_uri, label}` returns `{session_token, expires_at, account, device_id}`.
5. Store the token in the platform's secure store:

   | Platform | Store |
   |---|---|
   | iOS | Keychain with `AfterFirstUnlockThisDeviceOnly`, so a VoIP push can wake the app before unlock |
   | macOS | Keychain |
   | Android | Keystore-wrapped EncryptedSharedPreferences |
   | Windows | Credential Manager / DPAPI |
   | Linux | libsecret |

6. `PUT /v1/devices/current` with the platform, the name and the push tokens.

Each code is single-use and expires after 120 s, so exchange it exactly once.

## Carbons on the web

Open `/v1/auth/web/start?next=<path>`. The server does PKCE, the state cookie and the exchange, then sets the HttpOnly `ring_session` cookie.

## Silicons (CLI)

`silicon-accounts login --app ring -q | ring login --slt-stdin` exchanges the SLT through `POST /v1/auth/silicon`.

- The server requires `kind == silicon`.
- The server registers the silicon's Ting subscription. It mints a User verification proof from the fresh access token, with scope `subscriptions.register` for Ting.

A silicon's custodian can restrict which apps the silicon may use. If `ring` is not allowed, the Accounts CLI fails with `app_not_allowed`.

## Server rules

- Key every record on `account.uuid`. Store `id`, `display_name` and `pfp_url` from the token response, and keep them current from webhooks and from `GET /v1/apps/ring/users/{uuid}`.
- Seal Accounts refresh tokens with AES-256-GCM.
  - Introspect them when a WebSocket connects, then every 10 minutes. Introspection does not spend a refresh token.
  - Refresh only to mint proofs. Refresh under a per-session lock, and persist the rotated refresh token before using the access token.
- `membership.signed_out` reports that one Accounts sign-in ended (Ring revoking a single device sends it too). Each Ring session is its own sign-in, so the server introspects the account's sessions and ends only those that Accounts now reports inactive.
- `membership.access_removed` and `account.deleted` end every Ring session of that account.
- Resolve callees with `GET /v1/accounts/by-id/{id}` (Basic app auth). The account must be `status: active`.
- Ting sends use an App verification proof (`receiving_app: ting`, scope `tings.send`, lifetime up to 30 min). Cache it, and mint a fresh one when 3 minutes remain.
