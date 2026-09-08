const test=require('node:test');
const assert=require('node:assert/strict');
const {splitImageReferences,validateImageData,installHermesMedia}=require('../electron/agents/hermes-media.cjs');
const {normalizeHistory}=require('../electron/agents/hermes-events.cjs');
test('native persisted image directives retain order, quoted paths and caption',()=>{
  const rows=normalizeHistory([{id:1,role:'user',content:'Caption\n@image:`/synthetic/my photo.png`\n@image:/synthetic/second.png'}]);
  assert.deepEqual(rows.filter(x=>x.kind==='image').map(x=>x.path),['/synthetic/my photo.png','/synthetic/second.png']);
  assert.equal(rows[0].text,'Caption\n');
  assert.equal(splitImageReferences('Example @image:/not-a-directive.png')[0].kind,'text');
  assert.equal(splitImageReferences('@image:https://example.com/image.png')[0].kind,'text');
  assert.equal(normalizeHistory([{role:'assistant',content:'@image:/synthetic/test.png'}])[0].kind,'text');
});
test('media reads resolve item IDs in the bound conversation, never renderer paths',async()=>{
  const calls=[];
  const provider={descriptor:{capabilities:[]},operations:new Map(),get(input){
    if(input.agentId!=='a'||input.conversationId!=='chat')throw Error('Unknown conversation');
    return {agentId:'a',state:{items:[{id:'photo',kind:'image',path:'/synthetic/photo.png',name:'photo.png'}]}};
  },transport(agent){assert.equal(agent,'a');return {async request(...args){calls.push(args);return {data_url:'data:image/png;base64,YQ=='}}}}};
  installHermesMedia(provider);
  const read=provider.operations.get('conversations.media');
  const input={agentId:'a',conversationId:'chat',itemId:'photo'};
  await assert.rejects(read({...input,path:'/private/file'}),/field/);
  await assert.rejects(read({...input,itemId:'unknown'}),/not present/);
  await assert.rejects(read({...input,agentId:'b'}),/Unknown/);
  assert.equal(calls.length,0);
  assert.equal((await read(input)).name,'photo.png');
  assert.deepEqual(calls[0],['GET','/api/media',{profile:'a',query:{path:'/synthetic/photo.png'}}]);
});
test('preview only accepts bounded canonical raster data URLs',()=>{
  for(const data of ['https://example.com/a.png','data:image/svg+xml;base64,YQ==','data:text/html;base64,YQ==','data:image/png;base64,!!!!','data:image/png;base64,YR=='])assert.throws(()=>validateImageData(data));
});
