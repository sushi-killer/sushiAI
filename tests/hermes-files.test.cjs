const test=require('node:test'),assert=require('node:assert/strict');
const {installHermesFiles}=require('../electron/agents/hermes-files.cjs');
test('file saves use native scoped methods and reject stale edits and incomplete previews',async()=>{
  let content='Original',binary=false,truncated=false;const calls=[];
  const p={descriptor:{addons:[]},operations:new Map(),async agent(input){assert.equal(input.agentId,'a');return 'a';},transport(){return {async request(method,route,options){calls.push({method,route,options});assert.equal(options.profile,'a');
    if(route.endsWith('default-cwd'))return {cwd:'/synthetic'};
    if(route.endsWith('/list'))return {entries:[]};
    if(route.endsWith('read-text'))return {path:'/synthetic/a.txt',text:content,byteSize:content.length,binary,truncated};
    if(route.endsWith('write-text')){content=options.body.content;return {ok:true};}
  }}}};
  installHermesFiles(p);const call=(action,data={})=>p.operations.get('addons.files.'+action)({agentId:'a',path:'/synthetic/a.txt',...data});
  assert.equal((await call('list',{path:undefined})).path,'/synthetic');
  const opened=await call('read');content='External change';
  await assert.rejects(call('save',{revision:opened.revision,content:'Overwrite'}),/changed on disk/);
  assert.equal(calls.filter(c=>c.method==='POST').length,0);
  const fresh=await call('read');
  assert.equal((await call('save',{revision:fresh.revision,content:'Saved'})).text,'Saved');
  truncated=true;await assert.rejects(call('save',{revision:fresh.revision,content:'No'}),/complete text/);
  await assert.rejects(call('read',{path:'relative'}),/absolute/);
});

test('lossy native text previews cannot overwrite original bytes, while literal replacement characters remain editable',async()=>{
  let bytes=Buffer.from([0xff]),writes=0;
  const p={descriptor:{addons:[]},operations:new Map(),async agent(){return 'a';},transport(){return {async request(method,route,options){
    if(route.endsWith('read-text'))return {path:'/synthetic/encoding.txt',text:bytes.toString('utf8'),byteSize:bytes.length,binary:false,truncated:false};
    if(route.endsWith('read-data-url'))return {dataUrl:'data:text/plain;base64,'+bytes.toString('base64')};
    if(route.endsWith('write-text')){writes++;bytes=Buffer.from(options.body.content);return {ok:true};}
  }}}};
  installHermesFiles(p);const call=(action,extra={})=>p.operations.get('addons.files.'+action)({agentId:'a',path:'/synthetic/encoding.txt',...extra});
  const lossy=await call('read');assert.match(lossy.readOnlyReason,/not lossless UTF-8/);
  await assert.rejects(call('save',{revision:lossy.revision,content:'Changed'}),/not lossless/);assert.equal(writes,0);assert.deepEqual(bytes,Buffer.from([0xff]));
  bytes=Buffer.from('Valid \uFFFD character','utf8');
  const valid=await call('read');assert.equal(valid.readOnlyReason,'');
  assert.equal((await call('save',{revision:valid.revision,content:'Edited \uFFFD character'})).text,'Edited \uFFFD character');assert.equal(writes,1);
});

test('raster file previews use native byte reads and remain non-editable',async()=>{
  const calls=[];
  const p={descriptor:{addons:[]},operations:new Map(),async agent(){return 'a';},transport(){return {async request(method,route,options){
    calls.push({method,route,options});return {dataUrl:'data:image/png;base64,YQ=='};
  }}}};
  installHermesFiles(p);
  const input={agentId:'a',path:'/synthetic/photo.png'};
  const image=await p.operations.get('addons.files.read')(input);
  assert.equal(image.imageData,'data:image/png;base64,YQ==');assert.equal(image.byteSize,1);assert.equal(image.binary,true);
  assert.deepEqual(calls[0],{method:'GET',route:'/api/fs/read-data-url',options:{profile:'a',query:{path:input.path}}});
  await assert.rejects(p.operations.get('addons.files.save')({...input,revision:'forged',content:'Overwrite'}),/complete text/);
  assert.ok(calls.every(call=>call.method==='GET'));
});
