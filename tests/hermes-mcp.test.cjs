const test=require("node:test"),assert=require("node:assert/strict");
const {installHermesMcp}=require("../electron/agents/hermes-mcp.cjs");
function fixture(){
  const calls=[];
  const p={operations:new Map(),descriptor:{addons:[]},agent:async i=>i.agentId,transport:agent=>({request:async(method,path,input)=>{
    calls.push({agent,method,path,input});
    if(path==="/api/mcp/catalog")return {entries:[{name:"sample",required_env:[{name:"SERVICE_KEY",prompt:"Service key",required:true}],description:"Test",transport:"http"}]};
    if(path==="/api/mcp/catalog/install")return {ok:true,background:true,action:"mcp-install-sample-1234"};
    if(path.includes("/api/actions/"))return {running:false,exit_code:0,lines:["Synthetic installation complete"]};
    if(path.endsWith("/auth"))return {flow_id:`flow-${agent}`,status:"authorization_required",authorization_url:"https://example.com/authorize"};
    if(path.includes("/flows/"))return {status:"approved",tools:[]};
    if(method==="GET")return {servers:[{name:"example",env:{API_KEY:"private"},transport:"http",url:"https://example.com/mcp",enabled:true}]};
    if(method==="POST"&&path==="/api/mcp/servers")return {name:input.body.name,env:input.body.env};
    return {ok:true,tools:[]};
  }})};installHermesMcp(p);return {calls,call:(action,input)=>p.operations.get(`addons.mcp.${action}`)(input)};
}
test("connection reads exclude credential values and pin profile",async()=>{
  const {call,calls}=fixture();const r=await call("list",{agentId:"second"});
  assert.deepEqual(r.servers[0].environmentKeys,["API_KEY"]);assert.ok(!JSON.stringify(r).includes("private"));
  assert.equal(calls[0].input.profile,"second");
});
test("connection provisioning validates transport and keeps native API explicit",async()=>{
  const {call,calls}=fixture();
  await assert.rejects(call("create",{agentId:"a",id:"x",url:"file:///etc/passwd"}));
  await assert.rejects(call("create",{agentId:"a",id:"../x",command:"node"}));
  await assert.rejects(call("create",{agentId:"a",id:"x",command:"node",args:"bad"}));
  await assert.rejects(call("create",{agentId:"a",id:"x",url:"https://example.com",profile:"b"}));
  assert.equal(calls.length,0);
  await call("create",{agentId:"a",id:"x",url:"https://example.com/mcp",auth:"header",bearerToken:"synthetic"});
  assert.equal(calls[0].input.body.bearer_token,"synthetic");assert.equal(calls[0].input.body.profile,"a");
});
test("OAuth flow IDs remain in main and cannot be reused across profiles",async()=>{
  const {call,calls}=fixture();const flow=await call("authorize",{agentId:"a",id:"x"});
  assert.equal(flow.flow_id,undefined);assert.equal(flow.status,"authorization_required");
  const count=calls.length;assert.deepEqual(await call("authStatus",{agentId:"b",id:"x"}),{status:"none"});assert.equal(calls.length,count);
  assert.equal((await call("authStatus",{agentId:"a",id:"x"})).status,"approved");
  assert.match(calls.at(-1).path,/flow-a$/);
  await call("cancelAuth",{agentId:"a",id:"x"});
  assert.deepEqual(await call("authStatus",{agentId:"a",id:"x"}),{status:"none"});
});

test("catalog installation accepts only declared credentials and scopes job polling",async()=>{
 const {call,calls}=fixture();
 await assert.rejects(call("install",{agentId:"a",id:"sample",env:{UNRELATED:"value"}}),/Invalid catalog credential/);
 await assert.rejects(call("install",{agentId:"a",id:"sample",env:{}}),/Provide Service key/);
 assert.ok(!calls.some(c=>c.path==="/api/mcp/catalog/install"));
 assert.deepEqual(await call("install",{agentId:"a",id:"sample",env:{SERVICE_KEY:"synthetic"}}),{status:"running"});
 const count=calls.length;assert.deepEqual(await call("installStatus",{agentId:"b",id:"sample"}),{status:"none"});assert.equal(calls.length,count);
 const result=await call("installStatus",{agentId:"a",id:"sample"});assert.equal(result.status,"installed");assert.equal(result.exitCode,0);
});

test("tool access changes only the selected server and preserves empty allow lists",async()=>{
 const {call,calls}=fixture();
 await call("toolPolicy",{agentId:"a",id:"example",include:[],exclude:[],prompts:false,resources:true});
 const save=calls.at(-1);assert.equal(save.path,"/api/config");assert.equal(save.input.profile,"a");
 assert.deepEqual(save.input.body.config,{mcp_servers:{example:{tools:{include:[],exclude:[],prompts:false,resources:true}}}});
 await assert.rejects(call("toolPolicy",{agentId:"a",id:"missing",include:null,exclude:[],prompts:true,resources:true}),/no longer exists/);
 await call("toolPolicy",{agentId:"a",id:"example",include:null,exclude:["delete_*"],prompts:true,resources:true});
 assert.equal(calls.at(-1).input.body.config.mcp_servers.example.tools.include,null);
});

function editableFixture() {
  const configs = new Map([['a', {mcp_servers:{example:{command:'node',args:['old.js'],env:{KEEP:'hidden-one',CHANGE:'hidden-two',REMOVE:'hidden-three'},enabled:false,tools:{include:['read']},custom:{timeout:40}},other:{url:'https://example.com/other',headers:{Authorization:'Bearer hidden-other'}}}}],['b',{mcp_servers:{example:{command:'node',args:['different.js']}}}]]);
  const calls=[];
  const p={operations:new Map(),descriptor:{addons:[]},agent:async i=>i.agentId,transport:agent=>({request:async(method,path,input)=>{
    calls.push({agent,method,path,input});
    if(method==='GET'&&path==='/api/config') return structuredClone(configs.get(agent));
    if(method==='GET'&&path==='/api/mcp/servers') return {servers:Object.entries(configs.get(agent).mcp_servers).map(([name,r])=>({name,...r,transport:r.url?'http':'stdio'}))};
    if(method==='PUT'&&path==='/api/mcp/servers'){configs.get(agent).mcp_servers=structuredClone(input.body.servers);return {ok:true};}
    throw Error('Unexpected endpoint');
  }})};installHermesMcp(p);return {configs,calls,call:(action,input)=>p.operations.get(`addons.mcp.${action}`)(input)};
}
test('editing preserves hidden credentials, unrelated connections and custom settings through validated native save',async()=>{
  const {call,configs,calls}=editableFixture();
  const opened=await call('read',{agentId:'a',id:'example'});
  assert.ok(!JSON.stringify(opened).includes('hidden-'));
  await call('update',{agentId:'a',id:'example',revision:opened.revision,command:'node',args:['new.js'],env:{CHANGE:'replacement'},removeEnv:['REMOVE']});
  const rows=configs.get('a').mcp_servers;
  assert.deepEqual(rows.example.env,{KEEP:'hidden-one',CHANGE:'replacement'});
  assert.deepEqual(rows.example.args,['new.js']);
  assert.equal(rows.example.enabled,false);assert.deepEqual(rows.example.tools,{include:['read']});assert.deepEqual(rows.example.custom,{timeout:40});
  assert.equal(rows.other.headers.Authorization,'Bearer hidden-other');
  assert.deepEqual(configs.get('b').mcp_servers.example.args,['different.js']);
  assert.equal(calls.at(-1).path,'/api/mcp/servers');assert.equal(calls.at(-1).input.body.profile,'a');
});
test('editing detects stale revisions, cross-profile revisions and invalid replacements before writing',async()=>{
  const {call,configs,calls}=editableFixture();
  const opened=await call('read',{agentId:'a',id:'example'});
  const input={agentId:'a',id:'example',revision:opened.revision,command:'node',args:[],env:{}};
  await assert.rejects(call('update',{...input,agentId:'b'}),/changed since/);
  await assert.rejects(call('update',{...input,args:'not an array'}),/Arguments/);
  await assert.rejects(call('update',{...input,env:{KEEP:'new'},removeEnv:['KEEP']}),/separate variables/);
  configs.get('a').mcp_servers.example.args=['external.js'];
  await assert.rejects(call('update',input),/changed since/);
  assert.equal(calls.filter(c=>c.method==='PUT').length,0);
});
test('HTTP edits preserve header references and reject embedded credentials or transport switches',async()=>{
  const {call,configs,calls}=editableFixture();
  const opened=await call('read',{agentId:'a',id:'other'});
  const input={agentId:'a',id:'other',revision:opened.revision};
  await assert.rejects(call('update',{...input,url:'https://user:pass@example.com'}),/embedded credentials/);
  await assert.rejects(call('update',{...input,url:'https://example.com',command:'node'}),/only to command/);
  await call('update',{...input,url:'https://example.com/new'});
  assert.equal(configs.get('a').mcp_servers.other.url,'https://example.com/new');
  assert.equal(configs.get('a').mcp_servers.other.headers.Authorization,'Bearer hidden-other');
  assert.equal(calls.filter(c=>c.method==='PUT').length,1);
});
