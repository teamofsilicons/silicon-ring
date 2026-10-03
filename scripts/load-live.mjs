#!/usr/bin/env node
// Bounded paid-provider capacity check. Uses isolated identities and never publishes Ting events.
import assert from 'node:assert/strict';
import {mkdtemp, readFile, writeFile, stat, rm} from 'node:fs/promises';
import {tmpdir, platform} from 'node:os';
import {join, resolve} from 'node:path';
import {spawn, execFile} from 'node:child_process';
import {once} from 'node:events';
import {parseArgs, promisify} from 'node:util';
import {randomBytes, randomUUID} from 'node:crypto';

const {values:args}=parseArgs({options:{calls:{type:'string',default:'15'},seconds:{type:'string',default:'30'},url:{type:'string'},'secrets-file':{type:'string'},binary:{type:'string',default:'target/debug/ring-server'},pid:{type:'string'},report:{type:'string'},'prepare-fixtures':{type:'string'},'paid-provider-smoke':{type:'boolean'}}});
const count=Number(args.calls), seconds=Number(args.seconds);
assert(Number.isInteger(count)&&count>=1&&count<=15, 'calls must be 1–15');
assert(Number.isFinite(seconds)&&seconds>=10&&seconds<=120, 'seconds must be 10–120');
const delay=ms=>new Promise(r=>setTimeout(r,ms));
let fixture={test_app_secret:randomBytes(32).toString('hex'),org_id:'ring-live-load',identities:{}};
for(let n=0;n<count;n++)for(const kind of ['c','si'])fixture.identities[randomBytes(24).toString('hex')]={actor:`${kind}:live${n}`,org_id:fixture.org_id,display_name:`Live check ${n}`,admin:false};
if(args['prepare-fixtures']){
  const path=resolve(args['prepare-fixtures']);
  await writeFile(path,JSON.stringify(fixture,null,2),{mode:0o600,flag:'wx'});
  await writeFile(`${path}.tokens.private.json`,JSON.stringify(fixture.identities),{mode:0o600,flag:'wx'});
  console.log(`Prepared ${count} isolated carbon/silicon pairs`);process.exit(0);
}
assert(args['paid-provider-smoke'], 'Requires --paid-provider-smoke; creates actual OpenAI Live and Deepgram sessions');
let url=args.url, dir, server, pid=args.pid, timer, sampling, logs='', clockTicks;
if(url){
  assert(args['secrets-file'], 'Remote mode requires --secrets-file');
  assert(((await stat(args['secrets-file'])).mode&0o077)===0, 'Secrets file must have mode 0600');
  fixture=JSON.parse(await readFile(args['secrets-file'],'utf8'));
  const parsed=new URL(url);
  assert(parsed.protocol==='wss:'||(parsed.protocol==='ws:'&&['127.0.0.1','localhost'].includes(parsed.hostname)), 'Remote connections require WSS');
}else assert(process.env.OPENAI_API_KEY&&process.env.DEEPGRAM_API_KEY, 'Local mode requires OpenAI and Deepgram credentials');
const identities=Object.entries(fixture.identities);
const carbons=identities.filter(([,i])=>i.org_id===fixture.org_id&&i.actor.startsWith('c:'));
const silicons=identities.filter(([,i])=>i.org_id===fixture.org_id&&i.actor.startsWith('si:'));
assert(carbons.length>=count&&silicons.length>=count, 'Need an isolated carbon and silicon per call');
const clients=[],calls=[],errors=[],resources=[];
const silence=Buffer.alloc(960).toString('base64');
function issue(message){if(errors.length<100)errors.push(message);}
async function sample(){
  if(!pid)return;
  try{
    if(platform()==='linux'){
      clockTicks??=Number((await promisify(execFile)('getconf',['CLK_TCK'])).stdout.trim());
      const [s,status]=await Promise.all([readFile(`/proc/${pid}/stat`,'utf8'),readFile(`/proc/${pid}/status`,'utf8')]);
      const fields=s.slice(s.lastIndexOf(')')+2).split(' '),rss=/VmRSS:\s+(\d+)/.exec(status);
      if(rss)resources.push({at:performance.now(),rss:Number(rss[1]),cpu:(Number(fields[11])+Number(fields[12]))/clockTicks});
    }else{
      const {stdout}=await promisify(execFile)('ps',['-p',String(pid),'-o','rss=','-o','time=']);
      const [rss,time]=stdout.trim().split(/\s+/);
      if(rss&&time)resources.push({at:performance.now(),rss:Number(rss),cpu:time.split(':').reverse().reduce((n,v,i)=>n+Number(v)*60**i,0)});
    }
  }catch{}
}
class Client{
  constructor(ws){
    this.ws=ws;this.pending=new Map();this.events=[];this.seq=0;this.frames=0;this.audible=0;this.lastOutput=null;this.firstAudio=null;
    ws.addEventListener('message',({data})=>{
      const v=JSON.parse(data),p=this.pending.get(v.id);
      if(p){clearTimeout(p.timer);this.pending.delete(v.id);v.ok?p.resolve(v.result):p.reject(new Error(`${p.method}: ${v.error.code}`));return;}
      this.events.push(v);
      if(v.type==='media.audio'){
        assert.equal(Buffer.from(v.data.audio_base64,'base64').length,960);
        if(this.lastOutput!==null&&v.data.seq!==this.lastOutput+1)issue('Output audio sequence gap');
        this.lastOutput=v.data.seq;this.frames++;
        if(Buffer.from(v.data.audio_base64,'base64').some(x=>x!==0)){this.audible++;this.firstAudio??=performance.now();}
      }
      if(['representative.failed','transcription.failed','representative.error'].includes(v.type))issue(`${v.type}: ${v.data.code}`);
    });
  }
  request(method,params={}){return new Promise((resolve,reject)=>{
    const id=randomUUID(),timer=setTimeout(()=>{this.pending.delete(id);reject(new Error(`Timeout: ${method}`));},30000);
    this.pending.set(id,{resolve,reject,timer,method});this.ws.send(JSON.stringify({id,method,params}));
  });}
  send(){if(!this.stream)return;this.ws.send(JSON.stringify({type:'media.audio',data:{stream_id:this.stream.stream_id,seq:this.seq,offset_ms:this.seq*20,audio_base64:silence}}));this.seq++;}
  close(){for(const p of this.pending.values())clearTimeout(p.timer);this.pending.clear();this.ws.close();}
}
async function connect(identity){
  const ws=new WebSocket(url);await new Promise((r,j)=>{ws.addEventListener('open',r,{once:true});ws.addEventListener('error',()=>j(new Error('WebSocket connection failed')),{once:true});});
  const client=new Client(ws);clients.push(client);
  await client.request('protocol.hello',{versions:[1],realm:'test',org_id:fixture.org_id,test_app_secret:fixture.test_app_secret});
  client.session=await client.request('auth.login',{token:identity[0]});
  await client.request('events.subscribe',{});return client;
}
let elapsed=0;
try{
  if(!url){
    dir=await mkdtemp(join(tmpdir(),'ring-live-load-'));
    await writeFile(join(dir,'tokens.json'),JSON.stringify(fixture.identities),{mode:0o600});
    url='ws://127.0.0.1:18768/ws';
    server=spawn(resolve(args.binary),[],{env:{...process.env,RING_BIND:'127.0.0.1:18768',RING_DATA_DIR:dir,RING_ENV:'test',RING_TEST_APP_SECRET:fixture.test_app_secret,RING_TEST_TOKENS_FILE:join(dir,'tokens.json'),RING_DISABLE_PROVIDERS:'0',RING_TELEMETRY_ENABLED:'false'},stdio:['ignore','pipe','pipe']});
    server.stdout.on('data',b=>logs=(logs+b).slice(-8000));server.stderr.on('data',b=>logs=(logs+b).slice(-8000));pid=server.pid;
    for(let n=0;n<200;n++){try{if((await fetch('http://127.0.0.1:18768/health')).ok)break;}catch{}if(n===199)throw new Error('Server readiness timeout');await delay(50);}
  }
  timer=setInterval(()=>{for(const call of calls)call.carbon.send();},20);
  await Promise.all(Array.from({length:count},async(_,n)=>{
    const [carbon,silicon]=await Promise.all([connect(carbons[n]),connect(silicons[n])]);
    const init=await carbon.request('calls.init',{target:silicons[n][1].actor});
    const call={carbon,silicon,ringid:init.ringid,started:performance.now()};calls.push(call);
    await silicon.request('calls.accept',{ringid:init.ringid,context:'This is a short connection check. Greet once, then wait quietly. Do not invent a conversation.',start:'Hello. The connection check is ready.'});
    carbon.stream=await carbon.request('media.attach',{ringid:init.ringid,device_id:carbon.session.device_id,purpose:'call'});
  }));
  const readyDeadline=Date.now()+60000;
  while(calls.some(c=>c.carbon.audible===0)&&Date.now()<readyDeadline)await delay(200);
  assert(calls.every(c=>c.carbon.audible>0), `${calls.filter(c=>c.carbon.audible>0).length}/${count} representatives produced audible PCM`);
  console.log(`All ${count} Live representatives are speaking through their independent relays`);
  await sample();sampling=setInterval(()=>void sample(),1000);
  const started=performance.now();await delay(seconds*1000);elapsed=(performance.now()-started)/1000;await sample();
  clearInterval(sampling);sampling=null;
  for(const call of calls){
    const active=await call.carbon.request('calls.get',{ringid:call.ringid});
    assert.equal(active.state,'active','Call ended during provider load');
    const transcript=await call.carbon.request('transcript.list',{ringid:call.ringid});
    assert(transcript.items.some(e=>e.kind==='speech'), 'Representative transcript was not retained');
    await call.carbon.request('calls.cut',{ringid:call.ringid});
    call.carbon.stream=null;
    for(let n=0;n<100;n++){call.recording=await call.carbon.request('recordings.get',{ringid:call.ringid});if(call.recording.status==='ready')break;await delay(50);}
    if(call.recording.status!=='ready'||call.recording.coverage!=='full')issue(`Recording ${call.recording.status}/${call.recording.coverage}`);
  }
}catch(error){issue(error.message);}
finally{
  if(timer)clearInterval(timer);if(sampling)clearInterval(sampling);
  for(const call of calls)try{await call.carbon.request('calls.cut',{ringid:call.ringid});}catch{}
  for(const client of clients)client.close();
  if(server?.exitCode===null){const closed=once(server,'exit');server.kill('SIGINT');await Promise.race([closed,delay(5000)]);if(server.exitCode===null)server.kill('SIGKILL');}
  if(dir)await rm(dir,{recursive:true,force:true});
}
const first=resources[0],last=resources.at(-1);
const report={passed:errors.length===0,timestamp:new Date().toISOString(),scope:'Concurrent actual OpenAI Live and Deepgram sessions, continuous silent carbon microphones, generated representative speech and full recordings; IAM fixtures and Ting suppressed',calls:count,requested_seconds:seconds,elapsed_seconds:Math.round(elapsed*100)/100,server_resources:resources.length?{samples:resources.length,max_rss_mib:Math.round(Math.max(...resources.map(r=>r.rss))/1024*100)/100,cpu_percent_of_one_core:Math.round((last.cpu-first.cpu)/(last.at-first.at)*100000*100)/100}:null,results:calls.map(c=>({audible_frames:c.carbon.audible,output_frames:c.carbon.frames,input_frames:c.carbon.seq,first_audio_ms:c.carbon.firstAudio===null?null:Math.round(c.carbon.firstAudio-c.started),recording_status:c.recording?.status,recording_coverage:c.recording?.coverage,missing_intervals:c.recording?.missing_intervals})),errors};
if(args.report)await writeFile(resolve(args.report),JSON.stringify(report,null,2)+'\n');
console.log(JSON.stringify(report,null,2));process.exitCode=report.passed?0:1;
