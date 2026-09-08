const test=require('node:test'),assert=require('node:assert/strict');
const {installHermesCommands}=require('../electron/agents/hermes-commands.cjs');
test('skill dispatch is scoped and catalog-validated, sending only native expanded content',async()=>{
  const calls=[],session={agentId:'a',id:'chat',runtime:'runtime-a',state:{status:'idle'}};
  const p={descriptor:{capabilities:[]},operations:new Map(),get(){return session;},transport(){return {async rpc(agent,method,params){calls.push({agent,method,params});
    if(method==='commands.catalog')return {pairs:[['/example','Example skill'],['/reset','Reset']],skills:{'/example':{}},warning:''};
    if(method==='command.dispatch')return {type:'skill',message:'Native expanded content'};
  }}},async send(input,options){calls.push({input,options});return {status:'running'};}};
  installHermesCommands(p);const invoke=p.operations.get('conversations.runSkill');
  await assert.rejects(invoke({command:'/reset'}),/not available/);
  assert.equal(calls.some(call=>call.method==='command.dispatch'),false);
  await invoke({command:'/example',arguments:'Check the result'});
  const dispatch=calls.find(call=>call.method==='command.dispatch');
  assert.deepEqual(dispatch,{agent:'a',method:'command.dispatch',params:{session_id:'runtime-a',name:'example',arg:'Check the result'}});
  assert.deepEqual(calls.at(-1),{input:{agentId:'a',conversationId:'chat',text:'Native expanded content'},options:{displayText:'/example Check the result'}});
  session.state.status='running';await assert.rejects(invoke({command:'/example'}),/finish/);
});
