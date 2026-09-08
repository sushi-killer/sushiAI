const test=require("node:test"),assert=require("node:assert/strict");
const {installHermesAddons}=require("../electron/agents/hermes-addons.cjs");
function fixture(handler){
 const calls=[];const provider={operations:new Map(),descriptor:{addons:[]},agent:async i=>i.agentId,transport:p=>({request:async(method,path,input)=>{calls.push({p,method,path,input});return handler(method,path,input);}})};
 installHermesAddons(provider);return {calls,call:(name,input)=>provider.operations.get(`addons.schedules.${name}`)({agentId:"default",...input})};
}
test("manual scheduled runs return immediately and deduplicate while executing",async()=>{
 let resolve;const f=fixture(()=>new Promise(r=>resolve=r));
 assert.deepEqual(await f.call("trigger",{id:"job"}),{status:"running"});
 assert.deepEqual(await f.call("trigger",{id:"job"}),{status:"running"});
 assert.equal(f.calls.length,1);assert.equal(f.calls[0].input.timeoutMs,600000);
 resolve({id:"job",state:"scheduled",schedule:{kind:"interval",minutes:30}});
 await new Promise(r=>setImmediate(r));
 const status=await f.call("triggerStatus",{id:"job"});assert.equal(status.status,"completed");assert.equal(status.resource.data.scheduleExpression,"every 30m");
});
test("ambiguous timeout is not reported as a completed scheduled run",async()=>{
 const f=fixture(()=>Promise.reject(Object.assign(Error("deadline"),{code:"REQUEST_TIMEOUT"})));
 await f.call("trigger",{id:"job"});await new Promise(r=>setImmediate(r));
 assert.equal((await f.call("triggerStatus",{id:"job"})).status,"unknown");
});
test("optional schedule fields can be cleared without affecting other fields",async()=>{
 const f=fixture(()=>({id:"job",schedule:{kind:"once",run_at:"2030-01-01T00:00:00Z"}}));
 const r=await f.call("update",{id:"job",updates:{model:"",provider:"",deliver:""}});
 assert.deepEqual(f.calls[0].input.body.updates,{model:"",provider:"",deliver:""});
 assert.equal(r.resource.data.scheduleExpression,"2030-01-01T00:00:00Z");
});

test("native model confirmation is surfaced without applying or auto-confirming",async()=>{
 const calls=[];const p={operations:new Map(),descriptor:{addons:[]},agent:async i=>i.agentId,transport:()=>({request:async(method,path,input)=>{calls.push({method,path,input});return input.body.confirm_expensive_model?{ok:true,model:"test",provider:"custom"}:{ok:false,confirm_required:true,confirm_message:"Review this model selection"};}})};
 installHermesAddons(p);const change=i=>p.operations.get("addons.models.set")({agentId:"default",provider:"custom",model:"test",...i});
 const pending=await change({});assert.equal(pending.confirmRequired,true);assert.equal(calls.length,1);assert.equal(calls[0].input.body.confirm_expensive_model,false);
 const applied=await change({confirm:true});assert.equal(applied.ok,true);assert.equal(calls[1].input.body.confirm_expensive_model,true);
});
test("reasoning writes use the native profile-scoped setter with a closed vocabulary",async()=>{
 const calls=[];const p={operations:new Map(),descriptor:{addons:[]},agent:async i=>i.agentId,transport:()=>({rpc:async(profile,method,params)=>{calls.push({profile,method,params});return {value:params.value};}})};
 installHermesAddons(p);const set=value=>p.operations.get("addons.models.setReasoning")({agentId:"second",value});
 await assert.rejects(set("anything"),/Invalid reasoning/);assert.equal(calls.length,0);
 await set("high");assert.deepEqual(calls[0],{profile:"second",method:"config.set",params:{key:"reasoning",value:"high",scope:"global",profile:"second"}});
});
