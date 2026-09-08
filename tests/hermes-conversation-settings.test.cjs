const test = require("node:test"),
  assert = require("node:assert/strict");
const {
  installConversationSettings,
} = require("../electron/agents/hermes-conversation-settings.cjs");
function fixture() {
  const s = {
      runtime: "live-a",
      agentId: "a",
      state: { status: "idle", info: { model: "old", provider: "custom" } },
    },
    calls = [];
  const p = {
    operations: new Map(),
    descriptor: { capabilities: [] },
    get: (i) => {
      assert.equal(i.agentId, "a");
      assert.equal(i.conversationId, "stored");
      return s;
    },
    transport: () => ({
      rpc: async (profile, method, input) => {
        calls.push({ profile, method, input });
        return p.reply(method, input);
      },
    }),
    reply: async (method, input) =>
      method === "config.get" ? { value: "medium" } : { value: input.value },
  };
  installConversationSettings(p);
  return {
    p,
    s,
    calls,
    call: (action, input = {}) =>
      p.operations.get(`conversations.settings.${action}`)({
        agentId: "a",
        conversationId: "stored",
        ...input,
      }),
  };
}
test("conversation settings validate the runtime and explicitly avoid global settings", async () => {
  const { call, calls } = fixture();
  assert.equal((await call("read")).reasoning, "medium");
  await call("reasoning", { value: "high" });
  const change = calls.at(-1);
  assert.equal(change.profile, "a");
  assert.deepEqual(change.input, {
    key: "reasoning",
    value: "high",
    scope: "session",
    session_id: "live-a",
  });
  await call("model", { model: "sample/v2", provider: "custom" });
  assert.equal(
    calls.find((c) => c.method === "config.set" && c.input.key === "model")
      .input.value,
    "sample/v2 --provider custom --session",
  );
  assert.equal(
    calls.find((c) => c.method === "config.set" && c.input.key === "model")
      .input.confirm_expensive_model,
    false,
  );
});
test("model confirmation is surfaced; running turns and flag injection are rejected", async () => {
  const { call, p, s, calls } = fixture();
  p.reply = async (method) =>
    method === "config.set"
      ? { confirm_required: true, confirm_message: "Review price" }
      : {};
  assert.deepEqual(
    await call("model", { model: "sample", provider: "custom" }),
    { confirmRequired: true, message: "Review price" },
  );
  await assert.rejects(
    call("model", { model: "sample --global", provider: "custom" }),
    /without command flags/,
  );
  s.state.status = "running";
  const count = calls.length;
  await assert.rejects(call("reasoning", { value: "low" }), /Stop or finish/);
  assert.equal(calls.length, count);
});
test("missing runtime never falls through to native global config and locks are released", async () => {
  const { call, p, s, calls } = fixture();
  p.reply = async () => {
    throw Error("Session not found");
  };
  await assert.rejects(
    call("reasoning", { value: "low" }),
    /Session not found/,
  );
  assert.deepEqual(
    calls.map((c) => c.method),
    ["session.status"],
  );
  assert.equal(s.settingsPending, false);
});
test("successful model switches retain native warnings without reporting a failed save", async () => {
  const { p, call, calls } = fixture();
  p.reply = async (method) =>
    method === "config.set"
      ? { value: "sample", warning: "Metadata unavailable", scope: "session" }
      : {};
  const result = await call("model", { model: "sample" });
  assert.equal(result.ok, true);
  assert.equal(result.warning, "Metadata unavailable");
  assert.equal(calls.at(-1).input.value, "sample --session");
});

test("model switches preserve the chosen session reasoning through native setters", async () => {
  const { call, calls } = fixture();
  await call("model", { model: "replacement" });
  const writes = calls.filter((c) => c.method === "config.set");
  assert.deepEqual(
    writes.map((c) => c.input.key),
    ["model", "reasoning"],
  );
  assert.equal(writes[1].input.value, "medium");
  assert.equal(writes[1].input.scope, "session");
});

test('model suggestions are scoped to the conversation profile and contain no credentials', async () => {
 const {p,s,call}=fixture();let request;
 s.state.status='running';
 p.transport=()=>({request:async(method,path,input)=>{request={method,path,input};return {provider:'custom:example',api_key:'secret',providers:[{name:'custom:example',models:['first',{id:'second',api_key:'hidden'},'first'],api_key:'hidden'}]};}});
 assert.deepEqual(await call('options'),{defaultProvider:'custom:example',providers:[{name:'custom:example',models:['first','second']}]});
 assert.deepEqual(request,{method:'GET',path:'/api/model/options',input:{profile:'a',query:{explicit_only:1,include_unconfigured:0}}});
 assert.equal(s.settingsPending,undefined);
});
test('named custom providers are passed explicitly without accepting command flags',async()=>{
 const {call,calls}=fixture();await call('model',{model:'example',provider:'custom:example'});
 assert.equal(calls.find(c=>c.method==='config.set'&&c.input.key==='model').input.value,'example --provider custom:example --session');
 await assert.rejects(call('model',{model:'example',provider:'custom:example --global'}),/without command flags/);
});
