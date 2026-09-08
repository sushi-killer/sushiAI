const test=require('node:test'),assert=require('node:assert/strict');
const {installHermesRecovery,recoveryPolicy}=require('../electron/agents/hermes-recovery.cjs');
test('recovery policy matches native defaults and boolean strings',()=>{
 assert.equal(recoveryPolicy({}).enabled,true);
 for(const value of [false,'off','unexpected','0'])assert.equal(recoveryPolicy({desktop:{auto_continue:{enabled:value}}}).enabled,false);
 for(const value of [true,'yes',' ON ','1'])assert.equal(recoveryPolicy({desktop:{auto_continue:{enabled:value}}}).enabled,true);
});
test('recovery changes only the selected profile flag and rejects unrelated settings',async()=>{
 const calls=[],config={desktop:{auto_continue:{enabled:true,freshness_minutes:99,max_attempts:4}},api_key:'hidden'};
 const p={descriptor:{addons:[]},operations:new Map(),agent:async i=>i.agentId,transport:agent=>({request:async(method,path,input)=>{calls.push({agent,method,path,input});if(method==='GET')return config;config.desktop.auto_continue.enabled=input.body.config.desktop.auto_continue.enabled;return {ok:true};}})};
 installHermesRecovery(p);
 const update=i=>p.operations.get('addons.recovery.update')(i);
 await assert.rejects(update({agentId:'second',enabled:'false'}),/boolean/);
 await assert.rejects(update({agentId:'second',enabled:false,profile:'default'}),/Unknown/);
 assert.equal(calls.length,0);
 assert.deepEqual(await update({agentId:'second',enabled:false}),{enabled:false});
 const save=calls.find(c=>c.method==='PUT');assert.equal(save.agent,'second');assert.equal(save.input.profile,'second');
 assert.deepEqual(save.input.body.config,{desktop:{auto_continue:{enabled:false}}});
 assert.equal(config.desktop.auto_continue.freshness_minutes,99);assert.equal(config.api_key,'hidden');
 assert.deepEqual(await p.operations.get('addons.recovery.read')({agentId:'second'}),{enabled:false});
});
