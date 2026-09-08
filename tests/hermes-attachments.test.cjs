const test = require('node:test');
const assert = require('node:assert/strict');
const { validateAttachments, stageAttachments } = require('../electron/agents/hermes-attachments.cjs');
test('attachment validation rejects invalid bytes, oversized files and paths before RPC', () => {
  for (const files of [[{name:'../secret',data:'YQ=='}], [{name:'a',data:'!'}], [{name:'a',data:''}], Array(9).fill({name:'a',data:'YQ=='})])
    assert.throws(() => validateAttachments(files));
  assert.throws(() => validateAttachments([{name:'large',data:Buffer.alloc(1024*1024+1).toString('base64')}]));
  assert.equal(validateAttachments([{name:'tiny.png',data:'YQ=='}])[0].image, true);
});
test('native attachment routing scopes all requests to the bound runtime', async () => {
  const calls = [];
  const staged = await stageAttachments(async (method, params) => {
    calls.push({method,params});
    return {attached:true,path:'/synthetic/'+params.session_id,ref_text:'@file:notes.txt'};
  }, 'runtime-a', validateAttachments([{name:'photo.png',data:'YQ=='},{name:'notes.txt',data:'Yg=='}]));
  assert.equal(staged.text,'[Attached image: photo.png]\n@file:notes.txt');
  assert.deepEqual(calls.map(c=>c.method), ['image.attach_bytes','file.attach']);
  assert.ok(calls.every(c=>c.params.session_id==='runtime-a'));
  await staged.detach();
  assert.equal(calls[2].method,'image.detach');
});
test('partial upload failure detaches already queued images and never submits a prompt', async () => {
  const calls=[];
  await assert.rejects(stageAttachments(async(method,params)=>{
    calls.push(method);
    if(method==='file.attach') throw Error('Upload failed');
    return {attached:true,path:'/synthetic/image'};
  },'bound',validateAttachments([{name:'a.png',data:'YQ=='},{name:'a.txt',data:'Yg=='}])),/Upload failed/);
  assert.deepEqual(calls,['image.attach_bytes','file.attach','image.detach']);
});
