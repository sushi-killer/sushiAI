const test=require('node:test'),assert=require('node:assert/strict');
const {installHermesGit}=require('../electron/agents/hermes-git.cjs');
test('Git reads resolve project root, pin agent scope and reject escaping file paths',async()=>{
  const calls=[];let root='/synthetic/repo';
  const p={descriptor:{addons:[]},operations:new Map(),async agent(){return 'a';},transport(){return {async request(method,route,options){calls.push({method,route,options});assert.equal(method,'GET');assert.equal(options.profile,'a');
    if(route.endsWith('default-cwd'))return {cwd:'/synthetic/repo/nested'};
    if(route.endsWith('git-root'))return {root};
    if(route.endsWith('/status'))return {branch:'main',ahead:0,behind:0};
    if(route.endsWith('/list'))return {files:[{path:'note.txt',status:'M',added:1,removed:1}]};
    if(route.endsWith('file-diff'))return {diff:'-old\n+new'};
  }}}};
  installHermesGit(p);const call=(action,input={})=>p.operations.get('addons.git.'+action)({agentId:'a',...input});
  assert.equal((await call('status')).path,root);
  assert.equal((await call('diff',{file:'note.txt'})).diff,'-old\n+new');
  assert.deepEqual(calls.at(-1).options.query,{path:root,file:'note.txt'});
  for(const file of ['../outside','/absolute','a/../../outside','a\\..\\outside'])await assert.rejects(call('diff',{file}),/relative/);
  root=null;assert.equal((await call('status')).repository,false);
  await assert.rejects(call('diff',{file:'note.txt'}),/not in a Git repository/);
});
