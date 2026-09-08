// Native attachment delivery using disposable files and a local model endpoint.
const fs = require("node:fs/promises"),
  http = require("node:http"),
  assert = require("node:assert/strict");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
const { HermesTransport } = require("../electron/agents/hermes-transport.cjs");
(async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-session-settings-");
  let observedBody, requestedModel,
    ownedChild,
    stallNext = false,
    requests = 0;
  const server = http.createServer(async (req, res) => {
    res.setHeader("Content-Type", "application/json");
    if (req.url.endsWith("/models")) {
      res.end(JSON.stringify({ data: [{ id: "synthetic-primary" }] }));
      return;
    }
    if (req.method !== "POST" || !req.url.endsWith("/chat/completions")) {
      res.writeHead(404);
      res.end("{}");
      return;
    }
    let raw = "";
    for await (const chunk of req) raw += chunk;
    const body = JSON.parse(raw);
    observedBody = body; requestedModel = body.model;
    requests++;
    if (stallNext) {
      stallNext = false;
      return;
    }
    const result = {
      id: "synthetic-completion",
      object: "chat.completion",
      model: body.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "Synthetic reply." },
          finish_reason: "stop",
        },
      ],
      usage: { prompt_tokens: 10, completion_tokens: 3, total_tokens: 13 },
    };
    if (body.stream) {
      res.setHeader("Content-Type", "text/event-stream");
      res.end(
        `data: ${JSON.stringify({ ...result, object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: "Synthetic reply." }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
      );
    } else res.end(JSON.stringify(result));
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${server.address().port}/v1`;
  await fs.writeFile(
    home + "/config.yaml",
    JSON.stringify({
      model: {
        provider: "custom",
        default: "synthetic-primary",
        base_url: base,
        api_key: "synthetic-test",
      },
      agent: { reasoning_effort: "medium" },
      toolsets: [],
      enabled_toolsets: [],
      auxiliary: { background_review: { enabled: false } },
    }),
  );
  const makeProvider = () =>
    new HermesProvider({
      transportFactory: (o) =>
        new HermesTransport({
          ...o,
          spawn: (...args) => { ownedChild = require("node:child_process").spawn(...args); return ownedChild; },
          spawn: (...args) => {
            ownedChild = require("node:child_process").spawn(...args);
            return ownedChild;
          },
          env: {
            HERMES_HOME: home,
            OPENAI_BASE_URL: base,
            OPENAI_API_KEY: "synthetic-test",
            HERMES_SKIP_UPDATE_CHECK: "1",
          },
        }),
    });
  let p = makeProvider();
  try {
    await fs.mkdir(home+"/skills/synthetic-command",{recursive:true});
    await fs.writeFile(home+"/skills/synthetic-command/SKILL.md","---\nname: synthetic-command\ndescription: Synthetic test command\n---\nUse SYNTHETIC_SKILL_SENTINEL for this task.\n");
    await p.listAgents();
    await fs.writeFile(home+"/synthetic-editor.txt","Original text");
    const fileInput={agentId:"default",path:home+"/synthetic-editor.txt"};
    const files=(action,input)=>p.operations.get("addons.files."+action)(input);
    const listing=await files("list",{agentId:"default",path:home});
    assert.ok(listing.entries.some(entry=>entry.name==="synthetic-editor.txt"));
    const file=await files("read",fileInput);
    assert.equal(file.text,"Original text");
    const saved=await files("save",{...fileInput,revision:file.revision,content:"Edited text"});
    assert.equal(await fs.readFile(fileInput.path,"utf8"),"Edited text");
    await fs.writeFile(fileInput.path,"External edit");
    await assert.rejects(files("save",{...fileInput,revision:saved.revision,content:"Do not overwrite"}),/changed on disk/);
    assert.equal(await fs.readFile(fileInput.path,"utf8"),"External edit");
    const encodedPath=home+"/synthetic-encoding.txt";
    await fs.writeFile(encodedPath,Buffer.from([0xff]));
    const encodedInput={agentId:"default",path:encodedPath};
    const encodingPreview=await files("read",encodedInput);
    assert.ok(encodingPreview.binary || encodingPreview.readOnlyReason);
    await assert.rejects(files("save",{...encodedInput,revision:encodingPreview.revision,content:"Do not overwrite"}));
    assert.deepEqual(await fs.readFile(encodedPath),Buffer.from([0xff]));
    await fs.writeFile(encodedPath,"Literal \uFFFD character","utf8");
    const utf8Preview=await files("read",encodedInput);
    assert.equal(utf8Preview.readOnlyReason,"");
    await files("save",{...encodedInput,revision:utf8Preview.revision,content:"Edited \uFFFD character"});
    assert.equal(await fs.readFile(encodedPath,"utf8"),"Edited \uFFFD character");
    const repo=home+"/synthetic-repo";await fs.mkdir(repo);
    const exec=require("node:util").promisify(require("node:child_process").execFile);
    const git=(...args)=>exec("git",["-c","core.hooksPath=/dev/null","-c","commit.gpgSign=false","-C",repo,...args]);
    await git("init","-b","main");
    await fs.writeFile(repo+"/note.txt","Original line\n");
    await git("add","note.txt");
    await git("-c","user.name=Synthetic Test","-c","user.email=synthetic@example.invalid","commit","-m","Synthetic fixture");
    await fs.writeFile(repo+"/note.txt","Changed line\n");
    await fs.writeFile(repo+"/new.txt","New file\n");
    const gitRead=(action,extra={})=>p.operations.get("addons.git."+action)({agentId:"default",path:repo,...extra});
    const gitStatus=await gitRead("status");
    assert.equal(gitStatus.branch,"main");assert.equal(gitStatus.files.length,2);
    const diff=await gitRead("diff",{file:"note.txt"});assert.match(diff.diff,/-Original line/);assert.match(diff.diff,/\+Changed line/);
    assert.match((await gitRead("diff",{file:"new.txt"})).diff,/\+New file/);
    const chat = await p.create({agentId:"default",title:"Synthetic attachments"});
    const input = {agentId:"default",conversationId:chat.conversationId};
    const skillCatalog=await p.operations.get("conversations.skills")(input);
    assert.ok(skillCatalog.commands.some(command=>command.name==="/synthetic-command"));
    await p.operations.get("conversations.runSkill")({...input,command:"/synthetic-command",arguments:"Synthetic user task"});
    const skillDeadline=Date.now()+30000;
    while((!observedBody || ["sending","running","waiting"].includes(p.snapshot(input).status)) && Date.now()<skillDeadline)await new Promise(r=>setTimeout(r,100));
    assert.match(JSON.stringify(observedBody.messages),/SYNTHETIC_SKILL_SENTINEL/);
    assert.match(JSON.stringify(observedBody.messages),/Synthetic user task/);
    assert.ok(p.snapshot(input).items.some(item=>item.text==="/synthetic-command Synthetic user task"));
    observedBody=undefined;
    const png="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a6YQAAAAASUVORK5CYII=";
    const runtime=p.get(input).runtime;
    const rpc=(method,params)=>p.transport("default").rpc("default",method,params);
    const image=await rpc("image.attach_bytes",{session_id:runtime,filename:"synthetic.png",content_base64:png});
    assert.equal(image.attached,true);
    const fileImage=await files("read",{agentId:"default",path:image.path});
    assert.equal(fileImage.imageData,"data:image/png;base64,"+png);
    assert.equal(fileImage.binary,true);
    assert.equal((await fs.readFile(image.path)).toString("base64"),png);
    assert.equal((await rpc("image.detach",{session_id:runtime,path:image.path})).detached,true);
    await p.send({...input,text:"Read the attached note.",attachments:[{name:"synthetic.png",data:png},{name:"synthetic-note.txt",data:Buffer.from("SUSHIAI_ATTACHMENT_SENTINEL").toString("base64")}]});
    const deadline=Date.now()+30000;
    while(!observedBody && Date.now()<deadline) await new Promise(r=>setTimeout(r,100));
    assert.ok(observedBody, "model must receive the prompt");
    assert.match(JSON.stringify(observedBody.messages), /SUSHIAI_ATTACHMENT_SENTINEL|@file:/);
    while (["sending","running","waiting"].includes(p.snapshot(input).status) && Date.now()<deadline) await new Promise(r=>setTimeout(r,100));
    const liveImage=p.snapshot(input).items.find(item=>item.kind==="image");
    assert.ok(liveImage,"live attachment preview must exist");
    const live=await p.operations.get("conversations.media")({...input,itemId:liveImage.id});
    assert.equal(live.dataUrl,"data:image/png;base64,"+png);
    await p.close(); p=makeProvider(); await p.listAgents();
    const restored=await p.open(input);
    const savedImage=restored.items.find(item=>item.kind==="image");
    assert.ok(savedImage,"image reference must survive a cold restart");
    assert.equal((await p.operations.get("conversations.media")({...input,itemId:savedImage.id})).dataUrl,live.dataUrl);
    const names=await fs.readdir(home+"/attachments");
    assert.ok(names.includes("synthetic-note.txt"));
    assert.equal(await fs.readFile(home+"/attachments/synthetic-note.txt","utf8"),"SUSHIAI_ATTACHMENT_SENTINEL");
    console.log(JSON.stringify({passed:true,nativeFileStaging:true,nativeImageStagingAndDetach:true,modelReceivedReference:true,nativeImagePreviewAfterRestart:true,nativeFileEditor:true,staleFileSaveRejected:true,nativeGitReview:true,encodingPreserved:true,nativeFileImagePreview:true,nativeSkillCommand:true}));
  } finally {
    await p.close(); server.closeAllConnections(); await new Promise(r=>server.close(r));
    await fs.rm(home,{recursive:true,force:true,maxRetries:5,retryDelay:200});
  }
})().catch(e=>{console.error(e.message);process.exitCode=1;});
