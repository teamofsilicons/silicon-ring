# Native call service

Private Tauri plugin used by the Ring mobile apps. `configure` receives the authenticated Ring session and endpoint; `control` supports start, stop, mute, status, restore, and logout. The frontend delegates microphone/playback ownership to this plugin on mobile. Desktop builds return `mobile: false` and retain browser audio.

Swift uses PushKit, CallKit, AVAudioEngine, Keychain, and URLSessionWebSocketTask. Kotlin uses FirebaseMessagingService, self-managed Telecom connections, a foreground call service, AudioRecord/AudioTrack, Android Keystore, and OkHttp WebSocket. Both use Ring protocol v1 methods and 20 ms, PCM s16le mono 24 kHz packets. Push tokens are registered using `devices.update`.

Build and provisioning instructions are in [the native app README](../README.md).
