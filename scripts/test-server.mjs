#!/usr/bin/env node
// End-to-end protocol checks use the real server and persistence, with isolated IAM fixtures.
import assert from 'node:assert/strict';
import {mkdtemp, writeFile, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';

const directory=await mkdtemp(join(tmpdir(),'ring-contract-'));
const secret='isolated-ring-contract-app-secret';
const identities={alice:{actor:'c:alice',org_id:'test',display_name:'Alice',admin:true},bob:{actor:'c:bob',org_id:'recipient-org',display_name:'Bob',admin:true},bobOther:{actor:'c:bob',org_id:'recipient-other-org',display_name:'Bob elsewhere'},eve:{actor:'c:eve',org_id:'recipient-org',display_name:'Eve'},si:{actor:'si:assistant',org_id:'silicon-org',display_name:'Assistant'},si2:{actor:'si:second',org_id:'second-silicon-org',display_name:'Second'},carol:{actor:'c:carol',org_id:'third-org',display_name:'Carol'}};
await writeFile(join(directory,'tokens.json'),JSON.stringify(identities));
let server, output='';
const clients=[];
const port=Number(process.env.RING_TEST_PORT||18765);
async function start(){server=spawn(resolve('target/debug/ring-server'),[],{env:{...process.env,RING_BIND:`127.0.0.1:${port}`,RING_DATA_DIR:directory,RING_TEST_APP_SECRET:secret,RING_TEST_TOKENS_FILE:join(directory,'tokens.json'),RING_DISABLE_PROVIDERS:'1',RING_TELEMETRY_ENABLED:'false'},stdio:['ignore','pipe','pipe']});server.stdout.on('data',c=>output+=c);server.stderr.on('data',c=>output+=c);for(let i=0;i<100;i++){try{if((await fetch(`http://127.0.0.1:${port}/health`)).ok)return;}catch{}await new Promise(r=>setTimeout(r,50));}throw new Error(`Server did not start: ${output}`);}
async function stop(){if(server?.exitCode===null){const closed=once(server,'exit');server.kill('SIGINT');await closed;}}
class Client{
 constructor(socket){this.socket=socket;this.pending=new Map();this.events=[];this.frames=[];socket.addEventListener('message',ev=>{const v=JSON.parse(ev.data);if(v.id&&this.pending.has(v.id)){const {resolve,reject,timer}=this.pending.get(v.id);clearTimeout(timer);this.pending.delete(v.id);v.ok?resolve(v.result):reject(Object.assign(new Error(v.error.message),v.error));}else{this.events.push(v);if(v.type==='media.audio')this.frames.push(v.data);}});}
 request(method,params={},id=crypto.randomUUID()){return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{this.pending.delete(id);reject(new Error(`Timeout ${method}`))},8000);this.pending.set(id,{resolve,reject,timer});this.socket.send(JSON.stringify({id,method,params}));});}
 frame(type,data){this.socket.send(JSON.stringify({type,data}));}
 async ready(){await this.request('protocol.hello',{versions:[1],client:{name:'contract-tests',version:'0.1.0'},realm:'test',org_id:this.org||'test',test_app_secret:secret});}
 async login(token,org){this.org=org||identities[token]?.org_id||'test';await this.ready();this.session=await this.request('auth.login',{token});await this.request('events.subscribe',{});return this;}
 close(){this.socket.close();}
}
async function socket(){const ws=new WebSocket(`ws://127.0.0.1:${port}/ws`);await new Promise((res,rej)=>{ws.addEventListener('open',res,{once:true});ws.addEventListener('error',rej,{once:true});});const c=new Client(ws);clients.push(c);return c;}
async function login(token,org){return (await socket()).login(token,org);}
async function fails(fn,code){await assert.rejects(fn,e=>e.code===code,`expected ${code}`);}
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function waitFor(check,label){for(let n=0;n<100;n++){const value=await check();if(value)return value;await delay(50);}throw new Error(`Timed out waiting for ${label}`);}
const event=(client,type,ringid,actor)=>waitFor(()=>client.events.find(e=>e.type===type&&e.data?.ringid===ringid&&(!actor||e.data.actor===actor)),`${type} for ${client.session.actor}`);
async function download(client,asset_id){
 const asset=await client.request('assets.get',{asset_id});assert.equal(asset.complete,true);
 await waitFor(()=>client.events.some(e=>e.type==='assets.chunk'&&e.data.transfer_id===asset.transfer_id&&e.data.final),'complete authorized asset download');
 const frames=client.events.filter(e=>e.type==='assets.chunk'&&e.data.transfer_id===asset.transfer_id).map(e=>e.data).sort((a,b)=>a.seq-b.seq);
 frames.forEach((frame,index)=>assert.equal(frame.seq,index));
 const bytes=Buffer.concat(frames.map(frame=>Buffer.from(frame.data_base64,'base64')));assert.equal(bytes.length,asset.size_bytes);assert.equal(bytes.toString('ascii',0,4),'RIFF');return bytes;
}
let checks=0;
function checked(message){checks++;console.log(`ok ${checks} - ${message}`);}
try{
 await start();
 const unauth=await socket();await fails(()=>unauth.request('calls.list',{}),'HELLO_REQUIRED');await fails(()=>unauth.request('protocol.hello',{versions:[2]}),'PROTOCOL_UNSUPPORTED');await fails(()=>unauth.request('protocol.hello',{versions:[1],realm:'test',test_app_secret:'bad'}),'TEST_APP_AUTH_FAILED');checked('protocol negotiation and isolated test authentication');
 const a=await login('alice'),b=await login('bob'),eve=await login('eve'),si=await login('si'),si2=await login('si2'),carol=await login('carol');
 assert.notEqual(a.session.org_id,b.session.org_id);assert.notEqual(b.session.org_id,carol.session.org_id);
 assert.equal((await a.request('profile.get',{actor:'c:bob'})).display_name,'Bob');
 await a.request('config.set',{scope:'org',values:{'retention.calls_days':17}});await b.request('config.set',{scope:'org',values:{'retention.calls_days':91}});
 const call=await a.request('calls.init',{target:'c:bob'},'first-call');assert.equal(call.state,'ringing');assert.equal((await a.request('calls.init',{target:'c:bob'},'first-call')).ringid,call.ringid);await fails(()=>a.request('calls.init',{target:'c:carol'},'first-call'),'REQUEST_ID_REUSED');
 await event(b,'call.incoming',call.ringid);
 for(const method of ['calls.get','transcript.list','recordings.get','events.subscribe'])await fails(()=>eve.request(method,{ringid:call.ringid}),'FORBIDDEN');
 await fails(()=>eve.request('media.attach',{ringid:call.ringid,device_id:eve.session.device_id,purpose:'call'}),'FORBIDDEN');
 assert(!(await eve.request('calls.list')).items.some(c=>c.ringid===call.ringid));
 const otherRealm=await socket();await otherRealm.request('protocol.hello',{versions:[1],realm:'production',org_id:a.session.org_id});await fails(()=>otherRealm.request('auth.resume',{session_token:a.session.session_token,device_id:a.session.device_id}),'FORBIDDEN');
 for(const method of ['calls.get','transcript.list','recordings.get','events.subscribe'])await fails(()=>otherRealm.request(method,{ringid:call.ringid}),'AUTH_REQUIRED');
 checked('global-ID call and incoming event cross organizations; outsider and cross-realm session access denied');
 const b2=await login('bob');await b.request('calls.accept',{ringid:call.ringid});await event(a,'call.accepted',call.ringid);await fails(()=>b2.request('calls.accept',{ringid:call.ringid}),'INVITATION_UNAVAILABLE');const active=await a.request('calls.get',{ringid:call.ringid});assert.equal(active.state,'active');assert.equal(active.participants.length,2);checked('cross-org offer can be claimed by one owned device only');
 const ma=await a.request('media.attach',{ringid:call.ringid,device_id:a.session.device_id,purpose:'call'}),mb=await b.request('media.attach',{ringid:call.ringid,device_id:b.session.device_id,purpose:'call'});
 const positive=Buffer.alloc(960),negative=Buffer.alloc(960);for(let i=0;i<480;i++){positive.writeInt16LE(1000,i*2);negative.writeInt16LE(-200,i*2);}
 for(let seq=0;seq<15;seq++){a.frame('media.audio',{stream_id:ma.stream_id,seq,offset_ms:seq*20,audio_base64:positive.toString('base64')});b.frame('media.audio',{stream_id:mb.stream_id,seq,offset_ms:seq*20,audio_base64:negative.toString('base64')});await delay(20);}
 assert(a.frames.some(f=>Buffer.from(f.audio_base64,'base64').readInt16LE(0)===-200));assert(b.frames.some(f=>Buffer.from(f.audio_base64,'base64').readInt16LE(0)===1000));assert.equal(eve.frames.length,0);assert(!eve.events.some(e=>e.data?.ringid===call.ringid));checked('cross-org PCM relay mixes minus self and leaks no audio or call events to an outsider');
 await fails(()=>b.request('calls.handoff',{ringid:call.ringid,to_device_id:b2.session.device_id}),'DEVICE_NOT_READY');const mb2=await b2.request('media.attach',{ringid:call.ringid,device_id:b2.session.device_id,purpose:'call'});assert.equal(mb2.state,'standby');let transcript=await b.request('transcript.list',{ringid:call.ringid});await b2.request('calls.handoff',{ringid:call.ringid,to_device_id:b2.session.device_id});assert.equal((await b.request('transcript.list',{ringid:call.ringid})).items.length,transcript.items.length);checked('handoff requires prepared owned device and does not alter conversation history');
 assert.equal((await b.request('profile.get',{actor:'c:carol'})).display_name,'Carol');
 await b.request('calls.invite',{ringid:call.ringid,target:'c:carol'});await event(carol,'call.incoming',call.ringid);await carol.request('calls.accept',{ringid:call.ringid});await event(b,'call.accepted',call.ringid,'c:carol');
 assert.equal((await carol.request('calls.get',{ringid:call.ringid})).participants.length,3);
 await a.request('calls.cut',{ringid:call.ringid});assert.equal((await b.request('calls.get',{ringid:call.ringid})).state,'active');await b.request('calls.cut',{ringid:call.ringid});assert.equal((await carol.request('calls.get',{ringid:call.ringid})).state,'ended');await event(carol,'call.ended',call.ringid);
 const recording=await waitFor(async()=>{const r=await b.request('recordings.get',{ringid:call.ringid});return r.status==='ready'&&r;},'finalized cross-org recording');assert(recording.audio_asset_id);
 for(const participant of [a,b,carol]){assert((await participant.request('calls.list')).items.some(c=>c.ringid===call.ringid));assert((await participant.request('transcript.list',{ringid:call.ringid})).items.length>0);}
 const bobAudio=await download(b,recording.audio_asset_id),carolAudio=await download(carol,recording.audio_asset_id);assert(bobAudio.length>44);assert(carolAudio.length<bobAudio.length,'Late joiner must not receive the full earlier recording');
 await fails(()=>eve.request('assets.get',{asset_id:recording.audio_asset_id}),'FORBIDDEN');await fails(()=>otherRealm.request('assets.get',{asset_id:recording.audio_asset_id}),'AUTH_REQUIRED');
 assert.equal((await a.request('config.get',{scope:'org'})).values['retention.calls_days'],17);assert.equal((await b.request('config.get',{scope:'org'})).values['retention.calls_days'],91);
 checked('third-org invitation, events, history, transcript and clipped recording access preserve participant and org boundaries');
 await si.request('config.set',{scope:'actor',values:{'representative.default_context':'Assist with the launch.','representative.context_mode':'append'}});const prep=await si.request('calls.prepare',{action:'init',target:'c:alice',context:'Use concise replies.'});assert.equal(prep.approval_required,true);await fails(()=>si.request('calls.init',{target:'c:alice',preparation_id:prep.preparation_id}),'CONTEXT_APPROVAL_REQUIRED');await si.request('context.approve',{preparation_id:prep.preparation_id,valid_for_seconds:3600});const sc=await si.request('calls.init',{target:'c:alice',preparation_id:prep.preparation_id,start:'Hello'});await fails(()=>si.request('calls.init',{target:'c:bob'}),'SILICON_BUSY');await a.request('calls.accept',{ringid:sc.ringid});await fails(()=>si.request('calls.handoff',{ringid:sc.ringid,to_device_id:a.session.device_id}),'DEVICE_NOT_READY');checked('reviewed context approval and single-call silicon reservation');
 await b.request('config.set',{scope:'actor',values:{'voicemail.greetings.busy':{text:'The sender greeting must never be used for the recipient.'}}});
 await si.request('config.set',{scope:'actor',values:{'voicemail.greetings.busy':{text:'The recipient keeps their own greeting across organizations.'}}});
 const busy=await b.request('calls.init',{target:'si:assistant'});assert.equal(busy.state,'voicemail');const vm=await b.request('voicemail.begin',{ringid:busy.ringid,format:'audio'});assert.equal(vm.greeting.text,'The recipient keeps their own greeting across organizations.');
 const vstream=await b.request('media.attach',{ringid:busy.ringid,device_id:b.session.device_id,purpose:'voicemail',voicemail_id:vm.voicemail_id});b.frame('media.audio',{stream_id:vstream.stream_id,seq:0,offset_ms:0,audio_base64:positive.toString('base64')});b.frame('media.audio',{stream_id:vstream.stream_id,seq:2,offset_ms:40,audio_base64:positive.toString('base64')});await delay(100);assert.equal((await b.request('media.detach',{stream_id:vstream.stream_id,last_seq:2})).complete,false);await fails(()=>b.request('voicemail.commit',{voicemail_id:vm.voicemail_id}),'AUDIO_NOT_READY');await b.request('voicemail.abort',{voicemail_id:vm.voicemail_id});checked('cross-org voicemail uses recipient settings and missing chunks cannot be committed');
 const live=await b.request('calls.init',{target:'c:carol'});await carol.request('calls.accept',{ringid:live.ringid});
 const liveBob=await b.request('media.attach',{ringid:live.ringid,device_id:b.session.device_id,purpose:'call'});const liveCarol=await carol.request('media.attach',{ringid:live.ringid,device_id:carol.session.device_id,purpose:'call'});
 b.frame('media.audio',{stream_id:liveBob.stream_id,seq:0,offset_ms:0,audio_base64:positive.toString('base64')});
 await waitFor(()=>carol.frames.some(f=>f.stream_id===liveCarol.stream_id&&Buffer.from(f.audio_base64,'base64').readInt16LE(0)===1000),'baseline live Bob microphone');await delay(100);
 const bobOther=await login('bobOther');assert.equal(bobOther.session.actor,b.session.actor);assert.notEqual(bobOther.session.org_id,b.session.org_id);
 const vm2=await b.request('voicemail.begin',{ringid:busy.ringid,format:'audio'});const vstream2=await bobOther.request('media.attach',{ringid:busy.ringid,device_id:bobOther.session.device_id,purpose:'voicemail',voicemail_id:vm2.voicemail_id});
 const privateStart=carol.frames.length;
 for(let seq=1;seq<=10;seq++){b.frame('media.audio',{stream_id:liveBob.stream_id,seq,offset_ms:seq*20,audio_base64:positive.toString('base64')});await delay(20);}
 const privateFrames=carol.frames.slice(privateStart).filter(f=>f.stream_id===liveCarol.stream_id);assert(privateFrames.length>0);assert(privateFrames.every(f=>Buffer.from(f.audio_base64,'base64').every(byte=>byte===0)),'Private voicemail must mute the same global actor in every org');
 for(let seq=0;seq<3;seq++)bobOther.frame('media.audio',{stream_id:vstream2.stream_id,seq,offset_ms:seq*20,audio_base64:positive.toString('base64')});await delay(50);assert.equal((await bobOther.request('media.detach',{stream_id:vstream2.stream_id,last_seq:2})).complete,true);await b.request('voicemail.commit',{voicemail_id:vm2.voicemail_id});
 const resumedStart=carol.frames.length;b.frame('media.audio',{stream_id:liveBob.stream_id,seq:11,offset_ms:220,audio_base64:positive.toString('base64')});
 await waitFor(()=>carol.frames.slice(resumedStart).some(f=>f.stream_id===liveCarol.stream_id&&Buffer.from(f.audio_base64,'base64').readInt16LE(0)===1000),'live microphone resumes after private voicemail');await b.request('calls.cut',{ringid:live.ringid});
 checked('private voicemail from another org mutes the global actor live microphone and restores it on detach');
 await event(si,'voicemail.received',busy.ringid);assert((await si.request('voicemail.list',{})).items.some(v=>v.voicemail_id===vm2.voicemail_id));const received=await si.request('voicemail.get',{voicemail_id:vm2.voicemail_id});assert.equal(received.read,false);assert(received.audio_asset_id);
 assert((await download(si,received.audio_asset_id)).equals(await download(b,received.audio_asset_id)));
 await si.request('voicemail.mark',{voicemail_id:vm2.voicemail_id,read:true});assert(!(await si.request('voicemail.list',{})).items.some(v=>v.voicemail_id===vm2.voicemail_id));
 for(const outsider of [a,eve]){await fails(()=>outsider.request('voicemail.get',{voicemail_id:vm2.voicemail_id}),'FORBIDDEN');await fails(()=>outsider.request('assets.get',{asset_id:received.audio_asset_id}),'FORBIDDEN');assert(!outsider.events.some(e=>e.data?.voicemail_id===vm2.voicemail_id));}
 await fails(()=>otherRealm.request('voicemail.get',{voicemail_id:vm2.voicemail_id}),'AUTH_REQUIRED');checked('cross-org voicemail event, recipient list/read state and private audio access');
 await fails(()=>si.request('representative.send',{ringid:sc.ringid,kind:'commentary',text:'🙂'.repeat(161),delegation_id:null}),'INVALID_INPUT');await si.request('representative.send',{ringid:sc.ringid,kind:'thinking',text:'Checking the launch.',delegation_id:null});assert(!(await a.request('transcript.list',{ringid:sc.ringid})).items.some(e=>e.kind==='thinking'));assert((await si.request('transcript.list',{ringid:sc.ringid})).items.some(e=>e.kind==='thinking'));await si.request('calls.cut',{ringid:sc.ringid});checked('Unicode boundaries and private thinking visibility');
 await si.request('config.set',{scope:'actor',values:{'representative.default_context':'New instructions.'}});await fails(()=>si.request('calls.init',{target:'c:alice',preparation_id:prep.preparation_id}),'CONTEXT_CHANGED');await fails(()=>si.request('calls.prepare',{action:'init',target:'c:alice',context:'🙂'.repeat(401)}),'INVALID_INPUT');checked('edited defaults invalidate approvals without truncating text');
 const declined=await a.request('calls.init',{target:'si:second'});await fails(()=>si2.request('calls.decline',{ringid:declined.ringid}),'INVALID_INPUT');await si2.request('calls.decline',{ringid:declined.ringid,give_no_reason:true});assert.equal((await a.request('calls.get',{ringid:declined.ringid})).state,'voicemail');checked('silicon decline requires reason or explicit opt-out');
 const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a5K0AAAAASUVORK5CYII=','base64');const photo=await a.request('assets.begin',{purpose:'profile_photo',mime_type:'image/png',size_bytes:png.length});a.frame('assets.chunk',{asset_id:photo.asset_id,seq:0,data_base64:png.toString('base64')});await delay(50);await a.request('assets.complete',{asset_id:photo.asset_id});await fails(()=>b.request('profile.update',{photo_asset_id:photo.asset_id}),'FORBIDDEN');checked('asset uploads enforce ownership and completeness');
 const resumed=await socket();await resumed.ready();await resumed.request('auth.resume',{session_token:a.session.session_token,device_id:a.session.device_id});await a.request('devices.revoke',{device_id:a.session.device_id});await fails(()=>resumed.request('calls.list',{}),'AUTH_REQUIRED');checked('device revocation immediately invalidates session access');
 for(const c of clients)c.close();await delay(100);await stop();await start();const restored=await login('si');assert((await restored.request('voicemail.list',{unread:false})).items.some(v=>v.voicemail_id===vm2.voicemail_id));const hist=await restored.request('calls.get',{ringid:sc.ringid});assert.equal(hist.state,'ended');
 const restoredBob=await login('bob');assert((await restoredBob.request('calls.list')).items.some(c=>c.ringid===call.ringid));assert.equal((await restoredBob.request('calls.get',{ringid:call.ringid})).state,'ended');assert((await download(restoredBob,recording.audio_asset_id)).equals(bobAudio));
 checked('cross-org call history, recording permissions and voicemail survive a server restart');
 console.log(`PASS: ${checks} end-to-end checks`);
}finally{for(const c of clients)c.close();await delay(30);await stop();await rm(directory,{recursive:true,force:true});}
