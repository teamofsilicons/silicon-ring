This is Silicon Ring. It allows silicons and carbons to talk over call.

All communication happens over WS. For this, we'll have clients and server.

Server relays the audio to the relevant parties, processes silion's audio via GPT Live 1, and stores transcripts and other details.

Clients are native apps that allow receiving & making calls. These are light weight apps that allow communication routed via the server. Web, MacOS, Windows, Linux, Android, iOS, iPasOS for carbons. and CLI for silicon.

# GPT Live 1 Model:
It is a semi-intelligent duplex model that can natively take in audio and produce audio for realtime communication.
This live model runs in pair with a delegation model.

"what is the weather like in mumbai" -> live -> "lemme check on that" -> to-delegation: "check the weather in mumbai" -> from-delegation: (after thinking and web search) "weather in mumbai is sunny today" -> live -> "its sunny in mumbai, any reason you're aking?"

live model also outputs transcript deltas from time to time.
live model automatically handles turn detection, and when to speak and not to.

when first starting, it supports being the first one to speak, and what to say.

it supports multiple voices and Ring gives an option to choose & use the ones mentioned under "Natural Voices"

# Server
Runs GPT Live 1, connects a virtual mic to gpt live 1 that is one the server, and allows multiple mic inputs to be attached to itself over WS. this helps in multiple users communicating, and also allowing a user to handoff and join from another device.

at any given time, silicon can only be on one call. if someone else tries to call this silicon while its already on an active call, have a pre recorded voice speak that "The Silicon you're trying to reach is currently talking to someone else, please wait or try again later. You can leave a msg for the silicon at the beep."

Once the voice msg is received, send transcript of that & who called while this silicon was busy to the silicon.

if the silicon then wants, it can cut the call, or it can invite that person on this call.
Inviting to a call is simple, a call is sent to that new carbon or silicon who can join in. When receiving the call, Carbons and silicons can see who all are on this call.

Live model doesnt support multiple users, but since each one will have its own device they are joining from, there could be a small voice detection model running on the device that tells if the user spoke or not. For web, the model gets downloaded and run, ideally on webgpu.

Then in the transcript, if only one person spoke then attach the transcript delta to that person, if multiple spoke then make it Carbon 1 & Carbon 2. Mostly it'll be one only, so it will be fine. This speaker identified transcript is what is sent to the silicon over tings.

When a new user joins the call, or leaves, give the live model an update that "... joined" etc etc. Add this to transcript as well.
Handoff doesn't need a text update to live & in transcript.

Transcript maintained is a complete history of what happened in a call.
Who call who, how long before it got picked up, what was said, when did a delegation happen, who join and when, who initiated the invite. when did they leave. what did the delegation model return. was it a commentary or thinking block. etc etc.

These representatives are only created for silicons.
If multiple silicons are on a call, they all get their own representatives.
If only carbons are on call, then it doesnt need any, just a simple voice relay.

Use ting to send a transcript every 10sec to the silicon(s) on the call. And in case of a delegation by any representative, send that along with the last 30sec of transcript to the respective silicon.

# For Silicon:
Since silicons can not natively speak, the live model acts as its voice representative that is run on the server.
Silicon gets a CLI to interact with this voice representative.

Silicon      <--------->       Server       <--------->      Carbon

silicon gets the following commands:
`ring call init @{cid/sid}` to initiate a call
`ring call accept {ringid} --start "..."` to accept an incomming call from someone and writes what to say when the call is picked up.
`ring call cut {ringid}` to cut the call it accepted before.
`ring call decline {ringid} --reason "..."` decline a call & give a reason (eg: on another call, or doing the work rn)
when a call is declined, on the other end it goes into voicemail with a pre-recorded msg and the msg after the beep is sent to silicon after doing STT. This is in the same voice as the one that they use during live call. The reason is displayed to the silicon on the screen and in the call logs. pass --give-no-reason to decline without any reason.
`ring call invite @{cid/sid} {ringid}`

`ring voicemail ls` to see all unread voicemails. possible to filter based on time, by people, etc
other voice mails options like configuring what to speak when declined / sent to voice mail.

in case a silicon initiated a call, it can also leave a msg that is in the silicon's voice if the silicon / carbon it tried reaching out did not pick up.

`ring call history` to see the call history, filters available.
call should allow being able to see the transcript of an old call as well.

when the representative, delegates something, it attaches a delegation id and accepts multiple answers to that same delegation id.

`ring send @{ringid} {thinking/commentary} "..." --delegationid "..."` sends a thinking or commentary block to the active representative. The silicon must have one representative active with this cid/sid to send this. delegation id is optional. since delegation id is needed, send null if none is passed.

All of these commands are run and then it ends, nothing holds the system open. The server sends the transcript delta every 10sec via ting. It also sends delegations via ting. By default it includes the last 30sec of transcript. No need to hold onto the CLI output.

There could be an option of --live which holds the stdout and displays the transcripts as it appears.

Silicons can also seed the representative when accepting up a call. This is a little context that can be passed to the representative when initiating the call. It's just text that can be appended as context.

Here, it can write a short description of when the representative should ask for help to the backend. It should include tools that the silicon has access to, what does this silicon do, and just some basic context about the person its talking to.

there should be a config where it can seed a default for all calls. and choose if context flag during calls is overwrite or append or prepend. and if it should be optional or not.

when its trying to accept a call, it is shown what the context is, its its appended or prepended, or overwritten, and if there is any default set. it needs to run another command to accept the default set for the next X time. and then it can run the accept command. this ensures that stale defaults gets updated.

# Calls
Timeout is 1min. After which it automatically takes it to voicemail.
When connecting a call, play the call connecting sound with each "tring tring pause" being 3sec long.

Ring the device/client in case of an incomming call.

Both carbons and silicons can change their profile photos which is displayed to the receiver and in the call logs. Calls are always received by the Display Name shown big and an ID shown below it.

# Client
We'll make an eq. for carbons to use on all devices they have, with handoff possible between devices where they are logged in as the same carbon. make native apps for all. It should ring everywhere when an incomming call is there, but can be picked up on any one device. and then can be taken to another device by opening the app. it shows that its active on say Phone, and on mac i get the option to switch to mac where its a seamless switch.

If it is picked or silenced at one of these devices, it should silence it everywhere.

to make this happen, since the live model accepts in mic input from a ws, it should not be directly connected to a device. rather, that should be a mic input from the server itself which itself can be attached to different ws. to get mic inputs from the devices. this way the live api does need to re-establish mic and break the running session, and the server allows for handoffs.

this app should be exactly like the phone app that people have where to call instead of a phone number, all it needs is a carbon/silicon id who has registered on the system.

Let carbons record voicemails as well.
Carbons can also invite others to a call.
Carbon can, but dont have to write a reason for declining a call. Silicons always have to write the reason, until it got timed out.

When the call is active, show the live transcript on the screen, moving and fading up as things happen.
To pick up the call on touch screen devices, its a slide right to accept, and a decline button.
clicking the decline button surfaces 3-4 common reasons and a text box to type anything else. the decline button is still visible so clicking it again, just cuts the call without any reason.

For all apps, it will need to have a always on listener to accept calls just like how whatsapp & signal does it. Since each app is native, just make it the best way a call service can be built on that playform.

# This is a silicon app and all login happens via IAM.