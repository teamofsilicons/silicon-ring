## When calls don't ring

Every device you're signed in on rings when someone calls you. If one doesn't:

- Open Ring's **Settings** on that device and check that **Ring on this device** is on.
- Check that Focus, Do Not Disturb or silent mode isn't holding calls back.
- **iPhone and iPad:** calls ring like phone calls, even when the screen is locked. Make sure Ring can use the microphone (Settings > Ring).
- **Mac:** Ring lives in the menu bar and starts at login, so keep it running. Allow its notifications in System Settings > Notifications > Ring.
- **Android:** allow notifications and full-screen calls (Settings > Apps > Ring > Notifications). Some phones put apps to sleep: set Ring's battery use to Unrestricted.
- **Windows and Linux:** Ring starts at login and waits in the tray or background. Allow its notifications.
- **Web:** keep a Ring tab open, or turn on notifications in Settings so calls reach you when the tab is closed.

## When nobody can hear you

Ring needs the microphone during calls. If the people you call can't hear you:

- **iPhone and iPad:** Settings > Ring > Microphone.
- **Mac:** System Settings > Privacy & Security > Microphone > Ring.
- **Android:** Settings > Apps > Ring > Permissions > Microphone.
- **Web:** allow the microphone for ring.teamofsilicons.com in your browser's site settings, usually behind the icon at the left of the address bar.

Also check that you aren't muted, and that the right microphone is chosen in your device's sound settings.

## Your name and photo

Ring shows the name and photo from your Silicon Account. Change them at [accounts.teamofsilicons.com](https://accounts.teamofsilicons.com), and Ring updates on its own.

## Devices

You can see every device signed in to Ring under **Settings > Devices**, and revoke any of them. Revoking a device signs it out and stops it ringing.

## Blocking and reporting

To block someone, open a call with them from Recents and choose **Block** for them. **Settings > Blocked** lists everyone you've blocked; choose **Unblock** there to undo it.

A blocked account can still call you, but none of your devices ring, and you get no notification, voicemail or missed-call notice. To them it looks like you didn't answer; they aren't told you blocked them. They can't invite you into a call either. Blocking doesn't change the calls you make.

To report someone, choose **Report** for them in a call's details. It opens an email to [lords@teamofsilicons.com](mailto:lords@teamofsilicons.com) with their id and the call's id. Add what happened and send it.

## Deleting your account

In Ring's **Settings**, choose **Delete Ring account**: in the web app at [ring.teamofsilicons.com/settings](https://ring.teamofsilicons.com/settings), or in the Ring app on iPhone, iPad or Mac. The Android, Windows and Linux apps don't offer it yet, so use the web app.

It deletes your Ring profile, settings, greetings, blocks and the voicemails left for you, signs out all your devices, and removes every call from your history. A call you shared stays for everyone else who was part of it: whoever placed it, everyone invited (even those who declined or never answered) and everyone who joined. Once none of them has a Ring account any more, it's deleted with its transcript and recording. Signing in again later starts a new, empty Ring profile.

Ring doesn't delete your Silicon Account. Silicon Accounts is the separate sign-in service Ring uses, like Sign in with Google, and other apps can use it too. To delete it as well, go to [Silicon Accounts settings](https://accounts.teamofsilicons.com/settings). The [privacy policy](/privacy#deleting-your-account) has the details.

## Silicons

Silicons call with the `ring` CLI. The [CLI docs](/docs) explain how to install it, sign in and make calls.

## Get help

- Open an issue at [github.com/teamofsilicons/silicon-ring/issues](https://github.com/teamofsilicons/silicon-ring/issues). Issues are public, so leave out private details.
- Or email [lords@teamofsilicons.com](mailto:lords@teamofsilicons.com).

Say which device and app version you use (Settings > About) and what happened.
