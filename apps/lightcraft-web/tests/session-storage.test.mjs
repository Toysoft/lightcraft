import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
import {randomUUID} from 'node:crypto';
import {IDBFactory, IDBObjectStore} from 'fake-indexeddb';
const source=readFileSync(new URL('../session.js',import.meta.url),'utf8');
class Element extends EventTarget {
  children=[]; dataset={}; textContent=''; removed=false;
  append(...children){this.children.push(...children)}
  setAttribute(){} showModal(){this.open=true} close(){this.open=false} remove(){this.removed=true} click(){return this.onclick?.()}
}
const request=r=>new Promise((resolve,reject)=>{r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});
const settle=()=>new Promise(r=>setImmediate(r));
async function until(check){for(let i=0;i<100;i++){if(check())return;await settle()}assert.fail('condition did not settle')}
function browser({db=new IDBFactory(),storage=new Map(),navigation='navigate',locks=new Map(),app='photocraft'}={}){
 const events=new EventTarget(),body=new Element(),head=new Element();let clock=1000;
 const navigator={storage:{persist:async()=>true},locks:{request:async(name,opts,callback)=>{
  if(locks.has(name))return callback(null);
  locks.set(name,true);try{return await callback({name})}finally{locks.delete(name)}
 }}};
 const context=vm.createContext({console,location:{pathname:'/',reload(){}},document:{body,head,readyState:'complete',createElement:()=>new Element()},navigator,indexedDB:db,crypto:{randomUUID},performance:{now:()=>clock,getEntriesByType:()=>[{type:navigation}]},sessionStorage:{getItem:k=>storage.get(k),setItem:(k,v)=>storage.set(k,v),removeItem:k=>storage.delete(k)},addEventListener:events.addEventListener.bind(events),dispatchEvent:events.dispatchEvent.bind(events),Event,ArrayBuffer,Uint8Array,TextDecoder,Blob,URL,setTimeout:()=>0,clearTimeout(){}});
 vm.runInContext(source,context);
 return {api:context.CraftSession,body,head,db,storage,locks,events,tick:()=>{clock+=1000},dialog:()=>body.children.findLast(e=>e.open&&!e.removed),button:label=>body.children.flatMap(e=>e.children).flatMap(e=>e.children).find(e=>e.textContent===label)};
}
async function ready(b){await until(()=>b.api.ready())}
async function save(b,sig,bytes=[1,2,3]){b.tick();assert.equal(b.api.shouldSave(sig),true);b.api.save(new Uint8Array(bytes));await until(()=>!b.api.status().saving);assert.equal(b.api.status().error,null)}
async function records(db){const d=await request(db.open('lightcraft-web-sessions',1));return await request(d.transaction('sessions').objectStore('sessions').getAll())}
test('refresh restores exact checkpoint automatically including binary bytes',async()=>{
 const first=browser();await ready(first);await save(first,'one',[0,255,9]);
 // Simulate the old document closing and releasing its Web Lock.
 first.locks.clear();const next=browser({db:first.db,storage:first.storage,navigation:'reload',locks:first.locks});
 await until(()=>next.api.status().phase==='restore');assert.deepEqual([...next.api.take()],[0,255,9]);assert.equal(next.api.ready(),false);
 next.api.restored(true,'');assert.equal(next.api.ready(),true);
});
test('returning prompts, and Start fresh preserves previous recovery',async()=>{
 const first=browser();await ready(first);await save(first,'one');
 const next=browser({db:first.db});await until(()=>!!next.dialog());assert.equal(next.api.ready(),false);assert.equal(next.api.take().length,0);
 await next.button('Start fresh').click();await ready(next);await save(next,'empty',[]);
 assert.equal((await records(first.db)).length,2);
});
test('duplicating a browser tab cannot claim or overwrite its live writer',async()=>{
 const first=browser();await ready(first);await save(first,'one');
 const next=browser({db:first.db,storage:new Map(first.storage),navigation:'reload',locks:first.locks});
 await until(()=>!!next.dialog());assert.notEqual(next.api.status().id,first.api.status().id);
 await next.button('Restore session').click();next.api.take();next.api.restored(true,'');await save(next,'second');
 assert.equal((await records(first.db)).length,2);
});
test('both stacks stay opaque, last checkpoint is retained, and only committed writes say saved',async()=>{
 const b=browser();await ready(b);await save(b,'s1',[8,1]);b.tick();assert(b.api.shouldSave('s2'));b.api.save(new Uint8Array([8,2]));
 assert.equal(b.api.status().dirty,true);assert.equal(b.api.status().saving,true);
 await until(()=>!b.api.status().saving);const [r]=await records(b.db);
 assert.deepEqual([...new Uint8Array(r.bytes)],[8,2]);assert.deepEqual([...new Uint8Array(r.previous)],[8,1]);assert.equal(b.api.status().dirty,false);
});
test('restore failure never overwrites the saved session and offers previous checkpoint',async()=>{
 const b=browser();await ready(b);await save(b,'s1',[1]);await save(b,'s2',[2]);b.locks.clear();
 const next=browser({db:b.db,storage:b.storage,navigation:'reload',locks:b.locks});await until(()=>next.api.status().phase==='restore');next.api.take();next.api.restored(false,'Invalid payload');
 assert.equal(next.api.shouldSave('empty'),false);assert(next.button('Try previous checkpoint'));assert.deepEqual([...new Uint8Array((await records(b.db))[0].bytes)],[2]);
 await next.button('Try previous checkpoint').click();assert.deepEqual([...next.api.take()],[1]);
});
test('denied storage keeps app usable only after explicit continue',async()=>{
 const b=browser({db:{open(){throw Error('Storage denied')}}});await until(()=>!!b.dialog());assert.match(b.api.status().error,/Storage denied/);
 await b.button('Continue without recovery').click();assert(b.api.ready());assert.equal(b.api.shouldSave('x'),false);
});
test('pending change warns before refresh and unchanged state clears warning',async()=>{
 const b=browser();await ready(b);await save(b,'one');b.events.dispatchEvent(new Event('input'));assert.equal(b.api.status().dirty,true);
 const event=new Event('beforeunload',{cancelable:true});b.events.dispatchEvent(event);assert(event.defaultPrevented);
 b.api.shouldSave('one');assert.equal(b.api.status().dirty,false);
});

test('quota error aborts the entire transaction and retry saves the latest pending checkpoint',async()=>{
 const b=browser();await ready(b);await save(b,'s1',[1]);
 const put=IDBObjectStore.prototype.put;
 IDBObjectStore.prototype.put=function(...args){if(this.name==='sessions')throw new DOMException('Quota full','QuotaExceededError');return put.apply(this,args)};
 try {b.tick();assert(b.api.shouldSave('s2'));b.api.save(new Uint8Array([2]));await until(()=>!b.api.status().saving);assert.match(b.api.status().error,/Quota full/);assert(b.api.status().dirty);}
 finally{IDBObjectStore.prototype.put=put;}
 assert.deepEqual([...new Uint8Array((await records(b.db))[0].bytes)],[1]);
 await b.body.children.find(e=>e.className==='craft-session-status').click();await b.button('Retry saving').click();await until(()=>!b.api.status().saving);
 assert.deepEqual([...new Uint8Array((await records(b.db))[0].bytes)],[2]);
});

test('undo while a write is pending schedules the newest state after commit',async()=>{
 const b=browser();await ready(b);await save(b,'a',[1]);
 b.tick();assert(b.api.shouldSave('b'));b.api.save(new Uint8Array([2]));
 assert.equal(b.api.shouldSave('a'),false);
 let woke=false;b.events.addEventListener('craft:session-wake',()=>{woke=true});
 await until(()=>!b.api.status().saving);assert(woke);assert(b.api.status().dirty);
 await save(b,'a',[1]);assert.deepEqual([...new Uint8Array((await records(b.db))[0].bytes)],[1]);
});
