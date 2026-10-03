#!/usr/bin/env node
// Opt-in, bounded integration test. Uses paid OpenAI/Deepgram credentials from .env.
import assert from 'node:assert/strict';
import {mkdtemp, writeFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn} from 'node:child_process';
import {once} from 'node:events';

assert(process.argv.includes('--paid-provider-smoke'), 'Requires --paid-provider-smoke and node --env-file=.env');
assert(process.env.OPENAI_API_KEY && process.env.DEEPGRAM_API_KEY, 'OpenAI and Deepgram credentials are required');
const dir = await mkdtemp(join(tmpdir(), 'ring-live-'));
const secret = 'isolated-ring-live-provider-app-secret';
await writeFile(join(dir, 'tokens.json'), JSON.stringify({carbon: {actor:'c:smoke',org_id:'smoke',display_name:'Carbon'},silicon:{actor:'si:smoke',org_id:'smoke',display_name:'Silicon'}}), {mode:0o600});
const port = 18767;
const server = spawn(resolve('target/debug/ring-server'), [], {env:{...process.env,RING_BIND:`127.0.0.1:${port}`,RING_DATA_DIR:dir,RING_ENV:'test',RING_TEST_APP_SECRET:secret,RING_TEST_TOKENS_FILE:join(dir,'tokens.json'),RING_DISABLE_PROVIDERS:'0',RING_TELEMETRY_ENABLED:'false'},stdio:['ignore','pipe','pipe']});
let logs = '', timer;
server.stdout.on('data', b => logs = (logs + b).slice(-8000));
server.stderr.on('data', b => logs = (logs + b).slice(-8000));
const delay = ms => new Promise(r => setTimeout(r, ms));
const clients = [];
class Client {
  constructor(ws) {
    this.ws=ws; this.pending=new Map(); this.events=[]; this.audible=0;
    ws.addEventListener('message', ({data}) => {
      const v=JSON.parse(data), p=this.pending.get(v.id);
      if(p){clearTimeout(p.timer);this.pending.delete(v.id);v.ok?p.resolve(v.result):p.reject(new Error(`${v.error.code}: ${v.error.message}`));}
      else {this.events.push(v);if(v.type==='media.audio'&&Buffer.from(v.data.audio_base64,'base64').some(x=>x!==0))this.audible++;}
    });
  }
  request(method,params={},id=crypto.randomUUID()) {return new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error(`Timeout: ${method}`)),120000);this.pending.set(id,{resolve,reject,timer});this.ws.send(JSON.stringify({id,method,params}));});}
  frame(type,data){this.ws.send(JSON.stringify({type,data}));}
  close(){for(const p of this.pending.values())clearTimeout(p.timer);this.ws.close();}
}
async function connect(token){const ws=new WebSocket(`ws://127.0.0.1:${port}/ws`);await new Promise((r,j)=>{ws.addEventListener('open',r,{once:true});ws.addEventListener('error',j,{once:true});});const c=new Client(ws);clients.push(c);await c.request('protocol.hello',{versions:[1],realm:'test',org_id:'smoke',test_app_secret:secret});c.session=await c.request('auth.login',{token});await c.request('events.subscribe',{});return c;}
async function until(predicate, ms=30000){const deadline=Date.now()+ms;while(Date.now()<deadline){if(await predicate())return;await delay(200);}throw new Error('Provider event deadline exceeded');}
try {
  await until(async()=>{try{return(await fetch(`http://127.0.0.1:${port}/health`)).ok;}catch{return false;}},10000);
  // Synthetic carbon speech is generated through the separate TTS endpoint for microphone input.
  const response=await fetch('https://api.openai.com/v1/audio/speech',{method:'POST',headers:{Authorization:`Bearer ${process.env.OPENAI_API_KEY}`,'Content-Type':'application/json'},body:JSON.stringify({model:'gpt-4o-mini-tts',voice:'marin',input:'What is the weather in Mumbai today? Please ask your backend assistant to check.',response_format:'pcm'}),signal:AbortSignal.timeout(30000)});
  assert(response.ok, `Speech fixture HTTP ${response.status}`);
  const speech=Buffer.from(await response.arrayBuffer());
  const carbon=await connect('carbon'), silicon=await connect('silicon');
  const call=await carbon.request('calls.init',{target:'si:smoke'});
  await silicon.request('calls.accept',{ringid:call.ringid,context:'You are a concise voice representative. Delegate questions requiring current facts to your backend. Do not guess current weather. Use backend results when they arrive.',start:'Hello, I am ready to help.'});
  const message={ringid:call.ringid,kind:'thinking',text:'The carbon is running a short integration check.',delegation_id:null};
  const queued=await silicon.request('representative.send',message,'startup-message');
  assert.deepEqual(await silicon.request('representative.send',message,'startup-message'),queued);
  const stream=await carbon.request('media.attach',{ringid:call.ringid,device_id:carbon.session.device_id,purpose:'call'});
  let seq=0, speechOffset=-1;
  timer=setInterval(()=>{
    const pcm=Buffer.alloc(960);
    if(speechOffset>=0&&speechOffset<speech.length){speech.copy(pcm,0,speechOffset,speechOffset+960);speechOffset+=960;}
    carbon.frame('media.audio',{stream_id:stream.stream_id,seq,offset_ms:seq*20,audio_base64:pcm.toString('base64')});seq++;
  },20);
  await until(()=>carbon.audible>5);
  console.log('ok - Live representative speaks through actual mixed PCM');
  await until(()=>silicon.events.some(e=>e.type==='representative.delivery'&&e.data.message_id===queued.message_id&&e.data.status==='accepted'));
  console.log('ok - pre-start thinking message is delivered once and acknowledged');
  await delay(3000);
  speechOffset=0;
  const speechStart=seq*20;
  carbon.frame('media.speech',{stream_id:stream.stream_id,start_ms:speechStart,end_ms:speechStart+Math.ceil(speech.length/48)});
  await until(()=>carbon.events.some(e=>e.type==='transcript.delta'&&e.data.entry.data.source==='deepgram'&&/weather|Mumbai/i.test(e.data.entry.data.text)));
  const speechEntry=carbon.events.find(e=>e.type==='transcript.delta'&&e.data.entry.data.source==='deepgram'&&/weather|Mumbai/i.test(e.data.entry.data.text));
  assert(speechEntry.data.entry.data.speaker_ids.includes('c:smoke'), JSON.stringify({entry:speechEntry.data.entry.data,reported_speech_start:speechStart,reported_speech_end:speechStart+Math.ceil(speech.length/48),current_stream_offset:seq*20,stream_errors:carbon.events.filter(e=>e.type==='stream.error')}));
  console.log('ok - Deepgram live captions identify the carbon microphone');
  await until(()=>silicon.events.some(e=>e.type==='delegation.created'));
  const delegation=silicon.events.find(e=>e.type==='delegation.created').data;
  assert(!carbon.events.some(e=>e.type==='delegation.created'));
  await silicon.request('representative.send',{ringid:call.ringid,kind:'thinking',text:'Checking current conditions.',delegation_id:delegation.delegation_id});
  const answer=await silicon.request('representative.send',{ringid:call.ringid,kind:'commentary',text:'This is a test result. Weather data was not fetched.',delegation_id:delegation.delegation_id});
  await until(()=>silicon.events.some(e=>e.type==='representative.delivery'&&e.data.message_id===answer.message_id&&e.data.status==='accepted'));
  console.log('ok - private delegation accepts multiple correlated responses');
  clearInterval(timer);timer=null;
  await carbon.request('calls.cut',{ringid:call.ringid});
  await until(async()=> (await carbon.request('recordings.get',{ringid:call.ringid})).status==='ready');
  console.log('PASS - actual GPT Live, Deepgram, mixed audio, transcript and delegation flow');
} finally {
  if(timer)clearInterval(timer);
  for(const c of clients)c.close();
  await delay(100);
  if(server.exitCode===null){const closed=once(server,'exit');server.kill('SIGINT');await Promise.race([closed,delay(5000)]);if(server.exitCode===null)server.kill('SIGKILL');}
  await rm(dir,{recursive:true,force:true});
}
