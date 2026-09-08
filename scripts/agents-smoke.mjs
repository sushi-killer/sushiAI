import { _electron as electron } from "playwright";
import fs from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";

const profile=await fs.mkdtemp("/tmp/sushiai-agent-ui-");
const app=await electron.launch({args:["."],env:{...process.env,SUSHIAI_TEST_HEADLESS:"1",BRIDGE_DATA_DIR:profile}});
try{
  const page=await app.firstWindow();
  await page.waitForSelector(".mode-switch");
  const errors=[];page.on("pageerror",e=>errors.push(e.message));
  const toolsMenu=async()=>{const menu=page.locator(".composer-tools");if(await menu.count() && !(await menu.evaluate(el=>el.open)))await page.getByLabel("Message tools",{exact:true}).click();};
  const resource=async(name)=>{const button=page.getByRole("button",{name,exact:true});if(!(await button.isVisible()))await page.getByRole("button",{name:"Agent settings",exact:true}).click();await button.click();};

  await app.evaluate(({ipcMain,BrowserWindow})=>{
    const window=BrowserWindow.getAllWindows()[0];
    const snapshot={agentId:"demo",conversationId:"primary",title:"Launch workspace",status:"idle",info:{model:"Example model"},usage:{total_tokens:1820},requests:[],items:[
      {id:"u",kind:"text",role:"user",text:"Prepare the launch checklist and remember our review process."},
      {id:"image",kind:"image",role:"user",name:"Synthetic image.png"},
      {id:"r",kind:"reasoning",text:"I will inspect the checklist and identify the reusable steps.",status:"complete"},
      {id:"t",kind:"tool",name:"memory",effect:"Memory added",category:"memory",status:"complete",input:{action:"add",content:"Review changes before publishing."},output:{success:true}},
      {id:"a",kind:"text",role:"assistant",text:"The checklist is ready.\n\n- [x] Verify the build\n- [x] Save the review process\n- [ ] Review the release\n\nUse `npm test` to run the checks."},
      {id:"review",kind:"notice",name:"review.summary",text:"Self-improvement review: created launch-checklist skill"},
    ]};
    globalThis.agentSmokeCalls=[];
    let mediaAttempts=0, editorText="Synthetic file content", editorRevision="initial";
    let scheduledRun=false, schedulerEnabled=false, clarificationFailure=true;
    for(const channel of ["agent-providers","agent-call"])ipcMain.removeHandler(channel);
    ipcMain.handle("agent-providers",()=>[{apiVersion:1,id:"sample",name:"Sample provider",description:"Synthetic test data",capabilities:["conversations","conversation-settings","attachments","image-preview","skill-commands"],addons:[{id:"soul",name:"Instructions"},{id:"mcp",name:"Connections"},{id:"schedules",name:"Schedules"},{id:"models",name:"Models"},{id:"recovery",name:"Recovery"},{id:"files",name:"Files"},{id:"git",name:"Git"}]}]);
    ipcMain.handle("agent-call",(_,provider,operation,input)=>{
      globalThis.agentSmokeCalls.push({provider,operation,input});
      if(operation==="conversations.media" && mediaAttempts++===0)throw Error("Synthetic missing image");
      if(operation==="conversations.media")return {dataUrl:"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a6YQAAAAASUVORK5CYII="};
      if(operation==="conversations.skills")return {commands:[{name:"/synthetic-command",description:"Synthetic skill for local testing"}],warning:""};
      if(operation==="conversations.runSkill")return snapshot;
      if(operation==="addons.git.status")return {path:"/synthetic",repository:true,branch:"main",ahead:0,behind:0,files:[{path:"note.txt",status:"M",added:1,removed:1,staged:false}]};
      if(operation==="addons.git.diff")return {diff:"@@ -1 +1 @@\n-Original line\n+Changed line"};
      if(operation==="addons.files.list")return {path:"/synthetic",parent:"/",entries:[{name:"note.txt",path:"/synthetic/note.txt",isDirectory:false},{name:"encoding.txt",path:"/synthetic/encoding.txt",isDirectory:false},{name:"preview.png",path:"/synthetic/preview.png",isDirectory:false}],truncated:false};
      if(operation==="addons.files.read" && input.path.endsWith("preview.png"))return {path:input.path,text:"",revision:"",binary:true,truncated:false,byteSize:68,imageData:"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a6YQAAAAASUVORK5CYII="};
      if(operation==="addons.files.read" && input.path.endsWith("encoding.txt"))return {path:input.path,text:"\uFFFD",revision:"encoding",binary:false,truncated:false,byteSize:1,readOnlyReason:"This file is not lossless UTF-8. Editing is disabled to preserve its original bytes."};
      if(operation==="addons.files.read")return {path:"/synthetic/note.txt",text:editorText,revision:editorRevision,binary:false,truncated:false,byteSize:editorText.length};
      if(operation==="addons.files.save"){editorText=input.content;editorRevision="saved";return {path:input.path,text:editorText,revision:editorRevision,binary:false,truncated:false,byteSize:editorText.length};}
      if(operation==="addons.recovery.read")return {enabled:true};
      if(operation==="addons.recovery.update")return {enabled:input.enabled};
      if(operation==="addons.models.current")return {id:"main",kind:"model",data:{model:"sample-model",provider:"sample"}};
      if(operation==="addons.models.options")return {providers:[{name:"sample",models:["sample-model"]}]};
      if(operation==="addons.models.reasoning")return {value:"medium",display:"show"};
      if(operation==="addons.models.setReasoning")return {ok:true,value:input.value};
      if(operation==="addons.models.set")return input.confirm?{ok:true}:{ok:false,confirmRequired:true,confirmMessage:"Review the synthetic model choice."};
      if(operation==="addons.schedules.schedulerStatus")return {enabled:schedulerEnabled,status:schedulerEnabled?"ready":"stopped",gatewayRunning:false};
      if(operation==="addons.schedules.schedulerSet"){schedulerEnabled=input.enabled;return {enabled:schedulerEnabled,status:schedulerEnabled?"ready":"stopped"};}
      if(operation==="addons.schedules.list")return {resources:[{id:"schedule1",kind:"schedule",data:{name:"Synthetic scheduled review",scheduleDisplay:"Every hour"}}]};
      if(operation==="addons.schedules.read")return {id:"schedule1",kind:"schedule",data:{name:"Synthetic scheduled review",prompt:"Review synthetic data",scheduleExpression:"every 60m",schedule:{kind:"interval",minutes:60},state:"scheduled",deliver:"local"}};
      if(operation==="addons.schedules.trigger"){scheduledRun=true;return {status:"running"};}
      if(operation==="addons.schedules.triggerStatus")return {status:scheduledRun?"completed":"none"};
      if(operation==="addons.schedules.runs")return {runs:[{id:"synthetic-run",title:"Synthetic run",message_count:2,is_active:false}]};
      if(operation==="addons.mcp.catalog")return {entries:[{name:"sample-catalog",description:"Synthetic catalog entry",transport:"http",authType:"none",requiredEnv:[],url:"https://example.com/mcp",args:[],bootstrap:[],postInstall:"Ready for new sessions.",installed:false,enabled:false}]};
      if(operation==="addons.mcp.installStatus")return {status:"none"};
      if(operation==="addons.mcp.install")return {status:"installed"};
      if(operation==="addons.mcp.list")return {servers:[{id:"sample-mcp",name:"Sample connection",transport:"http",url:"https://example.com/mcp",args:[],environmentKeys:[],auth:"none",enabled:true}]};
      if(operation==="addons.mcp.read")return {id:"sample-mcp",name:"Sample connection",revision:"synthetic-revision",transport:"http",url:"https://example.com/mcp",args:[],environmentKeys:[],auth:"none",enabled:true};
      if(operation==="addons.mcp.authStatus")return {status:"none"};
      if(operation==="addons.mcp.test")return {ok:true,tools:[{name:"synthetic_check",description:"Synthetic check"}],prompts:0,resources:0};
      if(operation==="conversations.settings.options")return {defaultProvider:"custom",providers:[{name:"custom",models:["sample-model","sample-v2"]},{name:"custom:second",models:["second-model"]}]};
      if(operation==="conversations.settings.read")return {reasoning:"medium",model:"sample-model",provider:"custom"};
      if(operation==="conversations.settings.model")return input.confirm?{ok:true}:{confirmRequired:true,message:"Review this conversation model."};
      if(operation==="agents.list")return [{id:"demo",providerId:"sample",name:"Launch assistant",description:"Synthetic test agent",model:"Example model",modelProvider:"Sample",skillCount:3}];
      if(operation==="conversations.canonical"||operation==="conversations.open")return input.conversationId==="new"?{...snapshot,conversationId:"new",title:"New conversation",items:[]}:snapshot;
      if(operation==="conversations.create")return {...snapshot,conversationId:"new",title:"New conversation",items:[]};
      if(operation==="conversations.send"){
        snapshot.items.push({id:"new-user",kind:"text",role:"user",text:input.text});
        snapshot.status="waiting";
        snapshot.requests=[{id:"approval-one",kind:"approval",input:{command:"Run the synthetic check",choices:["once","deny"]}}];
        window.webContents.send("agent-event",{providerId:"sample",type:"conversation",...snapshot});
        return snapshot;
      }
      if(operation==="interactions.respond"){
        if(input.requestId==="approval-one"){
          snapshot.requests=[{id:"clarify-batch",kind:"clarify",input:{questions:[
            {qid:"goal",question:"What is the goal?"},{qid:"format",question:"Which format?"}
          ]}}];
        }else{
          if(input.response.questionId==="format" && clarificationFailure){
            clarificationFailure=false;throw Error("Synthetic connection interruption");
          }
          snapshot.requests[0].input.questions=snapshot.requests[0].input.questions.filter(q=>q.qid!==input.response.questionId);
          if(!snapshot.requests[0].input.questions.length){snapshot.status="idle";snapshot.requests=[];}
        }
        window.webContents.send("agent-event",{providerId:"sample",type:"conversation",...snapshot});return {resolved:true};
      }
      if(operation==="conversations.list")return {conversations:[{id:input.search?"found":"primary",agentId:"demo",title:input.search?"Found in older message":"Launch workspace",messageCount:8}],nextOffset:null};
      if(operation==="activity.list")return [{id:"activity1",agentId:"demo",agentName:"Launch assistant",conversationId:"primary",conversationTitle:"Launch workspace",title:"Skill created",summary:"launch-checklist",kind:"skills",createdAt:Date.now(),read:false}];
      if(operation==="addons.soul.read")return {id:"soul",kind:"soul",data:{content:"# Instructions\nHelp with release reviews."}};
      if(operation==="addons.soul.update")return {ok:true};
      return {ok:true};
    });
  });
  await page.getByRole("button",{name:"Agent",exact:true}).click();
  await page.getByRole("button",{name:/Launch assistant/}).click();
  await page.getByText("Memory added",{exact:true}).waitFor();
  await page.locator(".agent-image").scrollIntoViewIfNeeded();
  await page.getByRole("button",{name:"Retry image",exact:true}).click();
  await page.waitForFunction(()=>{const image=document.querySelector('.agent-image img');return image?.complete&&image.naturalWidth===1;});
  await page.locator(".agent-detail").first().locator("summary").click();
  assert.match(await page.locator(".agent-transcript").innerText(),/I will inspect/);
  await page.getByRole("button",{name:"Activity",exact:true}).click();
  await page.getByRole("heading",{name:"Agent activity"}).waitFor();
  await page.getByText("Skill created",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await resource("Instructions");
  await page.getByLabel("Content",{exact:true}).fill("# Revised synthetic instructions");
  await page.getByRole("button",{name:"Save",exact:true}).click();
  await page.getByRole("status").filter({hasText:"Saved"}).waitFor();
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await resource("Connections");
  await page.getByRole("button",{name:"Browse catalog",exact:true}).click();
  await page.getByRole("button",{name:/sample-catalog/}).click();
  await page.getByRole("button",{name:"Install connection",exact:true}).click();
  await page.getByText("Connection installed",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Back to connections",exact:true}).click();
  await page.getByRole("button",{name:/Sample connection/}).click();
  await page.getByRole("button",{name:"Edit connection",exact:true}).click();
  assert.equal(await page.getByLabel("Server URL",{exact:true}).inputValue(),"https://example.com/mcp");
  await page.getByLabel("Server URL",{exact:true}).fill("https://example.com/edited");
  await page.getByRole("button",{name:"Save connection",exact:true}).click();
  await page.getByRole("button",{name:"Edit connection",exact:true}).waitFor();
  await page.getByRole("button",{name:"Test connection",exact:true}).click();
  await page.getByText("Connected · 1 tools",{exact:true}).waitFor();
  await page.getByText("Tool access",{exact:true}).click();
  await page.getByLabel("Only allow listed tools",{exact:true}).check();
  await page.getByLabel("Allowed tools",{exact:true}).fill("synthetic_check");
  await page.getByRole("button",{name:"Save tool access",exact:true}).click();
  await page.getByText("Tool access saved. Changes apply to new agent sessions.",{exact:true}).waitFor();
  await page.getByText("Tool access",{exact:true}).click();

  await fs.mkdir("artifacts",{recursive:true});
  await page.screenshot({path:"artifacts/agents-connections.png"});
  const originalSize=await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].getSize());
  await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].setSize(600,440));
  const connectionLayout=await page.locator(".agent-addon-panel").evaluate(el=>({width:el.clientWidth,scroll:el.scrollWidth}));
  assert.ok(connectionLayout.scroll<=connectionLayout.width+1,JSON.stringify(connectionLayout));
  await page.screenshot({path:"artifacts/agents-connections-compact.png"});
  await app.evaluate(({BrowserWindow},size)=>BrowserWindow.getAllWindows()[0].setSize(...size),originalSize);

  await page.getByRole("button",{name:"Add connection",exact:true}).click();
  await page.getByLabel("Connection name",{exact:true}).fill("synthetic-new");
  await page.getByLabel("Server URL",{exact:true}).fill("https://example.com/test");
  await page.getByRole("button",{name:"Save connection",exact:true}).click();
  await page.getByText("Connection saved. Changes apply to new agent sessions.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await resource("Schedules");
  await page.getByRole("button",{name:"Enable in sushiAI",exact:true}).click();
  await page.getByRole("button",{name:"Disable in sushiAI",exact:true}).click();

  await page.getByRole("button",{name:/Synthetic scheduled review/}).click();
  assert.equal(await page.getByLabel("Schedule expression",{exact:true}).inputValue(),"every 60m");
  await page.getByRole("button",{name:"Run now",exact:true}).click();
  await page.getByText("Run completed",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Run history",exact:true}).click();
  await page.getByRole("button",{name:/Synthetic run/}).click();
  await resource("Models");
  await page.getByLabel("Default reasoning effort",{exact:true}).selectOption("high");
  await page.getByRole("button",{name:"Save reasoning effort",exact:true}).click();
  await page.getByText("Reasoning effort saved for new conversations.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Use model",exact:true}).click();
  await page.getByText("Review the synthetic model choice.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Confirm model change",exact:true}).click();
  await page.getByRole("button",{name:"Use model",exact:true}).waitFor();
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await page.getByRole("button",{name:"Conversation settings",exact:true}).click();
  await page.getByLabel("Conversation reasoning",{exact:true}).selectOption("low");
  await page.getByRole("button",{name:"Save conversation reasoning",exact:true}).click();
  await page.getByText("Reasoning saved for this conversation.",{exact:true}).waitFor();
  await page.waitForFunction(()=>{const input=document.querySelector('.agent-conversation-settings input[list$="-models"]');return input?.list?.options.length===2;});
  await page.getByLabel("Model provider (optional)",{exact:true}).fill("custom:second");
  assert.equal(await page.locator('.agent-conversation-settings datalist[id$="-models"] option').first().getAttribute('value'),"second-model");
  await page.getByLabel("Model provider (optional)",{exact:true}).fill("");
  await page.getByLabel("Conversation model",{exact:true}).fill("sample-v2");
  await page.getByRole("button",{name:"Save conversation model",exact:true}).click();
  await page.getByText("Review this conversation model.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Confirm conversation model",exact:true}).click();
  await page.getByText("Model saved for this conversation.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Conversation settings",exact:true}).click();
  await toolsMenu();
  await page.getByRole("button",{name:"Run a skill",exact:true}).click();
  await page.getByLabel("Search skill commands",{exact:true}).fill("synthetic");
  await page.getByRole("button",{name:/synthetic-command Synthetic skill/}).click();
  await page.getByLabel("Skill instructions",{exact:true}).fill("Check synthetic work");
  await page.getByRole("button",{name:"Run selected skill",exact:true}).click();
  await page.getByLabel("Skill commands",{exact:true}).waitFor({state:"hidden"});
  const invocation=await app.evaluate(()=>globalThis.agentSmokeCalls.find(call=>call.operation==="conversations.runSkill"));
  assert.deepEqual(invocation.input,{agentId:"demo",conversationId:"primary",command:"/synthetic-command",arguments:"Check synthetic work"});
  await resource("Files");
  await page.getByRole("button",{name:"note.txt",exact:true}).click();
  await page.getByLabel("File contents",{exact:true}).fill("Edited synthetic content");
  await page.getByRole("button",{name:"Code",exact:true}).click();
  await page.getByRole("button",{name:"Agent",exact:true}).click();
  await resource("Files");
  assert.equal(await page.getByLabel("File contents",{exact:true}).inputValue(),"Edited synthetic content");
  await page.getByRole("button",{name:"Save file",exact:true}).click();
  await page.getByText("File saved.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"preview.png",exact:true}).click();
  await page.waitForFunction(()=>{const image=document.querySelector('.agent-file-editor img');return image?.complete&&image.naturalWidth===1;});
  assert.equal(await page.getByLabel("File contents",{exact:true}).count(),0);
  await page.getByRole("button",{name:"encoding.txt",exact:true}).click();
  await page.getByText("This file is not lossless UTF-8. Editing is disabled to preserve its original bytes.",{exact:true}).waitFor();
  assert.equal(await page.getByLabel("File contents",{exact:true}).evaluate(el=>el.readOnly),true);
  assert.equal(await page.getByRole("button",{name:"Save file",exact:true}).isDisabled(),true);
  await page.getByRole("button",{name:"note.txt",exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('textarea[aria-label="File contents"]')?.value==="Edited synthetic content");
  await page.screenshot({path:"artifacts/agents-files.png"});
  await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].setSize(600,600));
  const fileLayout=await page.locator(".agent-files-panel").evaluate(el=>({width:el.clientWidth,scroll:el.scrollWidth}));
  assert.ok(fileLayout.scroll<=fileLayout.width+1,JSON.stringify(fileLayout));
  await page.screenshot({path:"artifacts/agents-files-compact.png"});
  await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].setSize(1200,850));
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await resource("Git");
  await page.getByRole("button",{name:/M note.txt/}).click();
  await page.getByLabel("File diff",{exact:true}).waitFor();
  assert.match(await page.getByLabel("File diff",{exact:true}).innerText(),/\+Changed line/);
  await page.screenshot({path:"artifacts/agents-git.png"});
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await resource("Recovery");
  await page.waitForFunction(()=>{const checkbox=document.querySelector('.agent-addon-panel input[type="checkbox"]');return checkbox&&!checkbox.disabled;});
  assert.equal(await page.getByLabel("Automatically continue interrupted tasks",{exact:true}).isChecked(),true);
  await page.getByLabel("Automatically continue interrupted tasks",{exact:true}).uncheck();
  await page.getByRole("button",{name:"Save recovery settings",exact:true}).click();
  await page.getByText("Recovery settings saved.",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Back to chat",exact:true}).click();
  await toolsMenu();
  // Exercise browser-native paste/drop payloads without reading the real clipboard.
  await page.getByLabel("Message agent").evaluate(el=>{
    const data=new DataTransfer(); data.items.add(new File(["synthetic image"],"clipboard.png",{type:"image/png"}));
    el.dispatchEvent(new ClipboardEvent("paste",{clipboardData:data,bubbles:true,cancelable:true}));
  });
  await page.getByRole("button",{name:"Remove clipboard.png",exact:true}).waitFor();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove clipboard.png",exact:true}).click();
  await page.locator(".agent-composer").evaluate(el=>{
    const data=new DataTransfer(); data.items.add(new File(["synthetic drop"],"dropped.txt"));
    el.dispatchEvent(new DragEvent("drop",{dataTransfer:data,bubbles:true,cancelable:true}));
  });
  await page.getByRole("button",{name:"Remove dropped.txt",exact:true}).waitFor();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove dropped.txt",exact:true}).click();
  await toolsMenu();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"oversized.txt",mimeType:"text/plain",buffer:Buffer.alloc(1024*1024+1)});
  await page.getByText("Choose non-empty files, no larger than 1 MB each.",{exact:true}).waitFor();
  // Pause FileReader, switch tabs, then complete the upload into its original draft.
  const originalTab=await page.getByRole("tab",{selected:true}).innerText();
  await page.evaluate(()=>{
    const original=FileReader.prototype.readAsDataURL;
    FileReader.prototype.readAsDataURL=function(file){
      window.finishAttachmentRead=()=>original.call(this,file);
      FileReader.prototype.readAsDataURL=original;
    };
  });
  await toolsMenu();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"delayed.txt",mimeType:"text/plain",buffer:Buffer.from("Delayed attachment")});
  await page.getByText("Reading files…",{exact:true}).waitFor();
  assert.equal(await page.getByRole("button",{name:"Send ↑",exact:true}).isDisabled(),true);
  await page.getByRole("button",{name:"New conversation",exact:true}).click();
  await page.getByRole("tab",{name:"New conversation",exact:true}).waitFor();
  await page.evaluate(()=>window.finishAttachmentRead());
  await page.getByRole("tab",{name:originalTab,exact:true}).click();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove delayed.txt",exact:true}).waitFor();
  await page.getByRole("tab",{name:"New conversation",exact:true}).click();
  await page.getByRole("tab",{name:"New conversation",exact:true,selected:true}).waitFor();
  await page.getByRole("button",{name:"Remove delayed.txt",exact:true}).waitFor({state:"hidden"});
  await page.getByRole("tab",{name:originalTab,exact:true}).click();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove delayed.txt",exact:true}).click();
  await page.evaluate(()=>{
    const original=FileReader.prototype.readAsDataURL;
    FileReader.prototype.readAsDataURL=function(file){window.finishAttachmentRead=()=>original.call(this,file);FileReader.prototype.readAsDataURL=original;};
  });
  await toolsMenu();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"mode-switch.txt",mimeType:"text/plain",buffer:Buffer.from("Mode-safe draft")});
  await page.getByText("Reading files…",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Code",exact:true}).click();
  await page.evaluate(()=>window.finishAttachmentRead());
  await page.getByRole("button",{name:"Agent",exact:true}).click();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove mode-switch.txt",exact:true}).waitFor();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove mode-switch.txt",exact:true}).click();
  await page.locator(".agent-composer").evaluate(el=>{
    const data=new DataTransfer();
    for(let i=0;i<9;i++)data.items.add(new File(["x"],`excess-${i}.txt`));
    el.dispatchEvent(new DragEvent("drop",{dataTransfer:data,bubbles:true,cancelable:true}));
  });
  await page.getByText("Attach up to 8 files per message.",{exact:true}).waitFor();
  assert.equal(await page.getByRole("button",{name:"Remove excess-0.txt",exact:true}).count(),0);

  await toolsMenu();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"synthetic-note.txt",mimeType:"text/plain",buffer:Buffer.from("Synthetic attachment")});
  await page.getByRole("button",{name:"Remove synthetic-note.txt",exact:true}).waitFor();
  await toolsMenu();
  await page.getByRole("button",{name:"Remove synthetic-note.txt",exact:true}).click();
  await toolsMenu();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"synthetic-note.txt",mimeType:"text/plain",buffer:Buffer.from("Synthetic attachment")});
  await page.getByRole("button",{name:"Remove synthetic-note.txt",exact:true}).waitFor();
  await page.getByLabel("Message agent").fill("Test approval routing");
  await page.getByRole("button",{name:"Send ↑",exact:true}).click();
  const attached=await app.evaluate(()=>globalThis.agentSmokeCalls.find(c=>c.operation==="conversations.send").input.attachments);
  assert.deepEqual(attached,[{name:"synthetic-note.txt",data:Buffer.from("Synthetic attachment").toString("base64")}]);
  await page.getByRole("button",{name:"Allow once",exact:true}).click();
  await page.getByText("Approval needed",{exact:true}).waitFor({state:"hidden"});
  await page.getByLabel("What is the goal?",{exact:true}).fill("Release");
  await page.getByLabel("Which format?",{exact:true}).fill("Markdown");
  await page.getByRole("button",{name:"Send response",exact:true}).click();
  await page.locator(".agent-interaction [role=alert]").waitFor();
  assert.equal(await page.getByLabel("Which format?",{exact:true}).inputValue(),"Markdown");
  assert.equal(await page.getByLabel("What is the goal?",{exact:true}).count(),0);
  await page.getByRole("button",{name:"Send response",exact:true}).click();
  await page.getByText("Your input is needed",{exact:true}).waitFor({state:"hidden"});
  await page.getByRole("button",{name:"Dismiss error",exact:true}).click();
  await page.getByLabel("Message agent").fill("Unsent draft survives mode switch");
  await page.getByRole("button",{name:"Code",exact:true}).click();
  await page.getByRole("button",{name:"Agent",exact:true}).click();
  await page.getByLabel("Message agent").waitFor();
  assert.equal(await page.getByLabel("Message agent").inputValue(),"Unsent draft survives mode switch");
  await page.getByRole("button",{name:"History",exact:true}).click();
  await page.getByLabel("Search conversations").fill("older");
  await page.getByText("Found in older message",{exact:true}).waitFor();
  await page.locator(".agent-history").getByRole("button",{name:"Close",exact:true}).click();
  await page.getByLabel("Message agent").fill("");
  await fs.mkdir("artifacts",{recursive:true});
  await page.screenshot({path:"artifacts/agents-desktop.png"});
  await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].setSize(600,440));
  await page.waitForTimeout(100);
  const layout=await page.locator(".agents-view").evaluate(el=>({width:el.clientWidth,scroll:el.scrollWidth}));
  assert.ok(layout.scroll<=layout.width+1,JSON.stringify(layout));
  await page.screenshot({path:"artifacts/agents-compact.png"});
  const calls=await app.evaluate(()=>globalThis.agentSmokeCalls);
  const edited=calls.find(c=>c.operation==="addons.mcp.update");
  assert.deepEqual(edited.input,{agentId:"demo",id:"sample-mcp",revision:"synthetic-revision",url:"https://example.com/edited"});
  const approval=calls.find(c=>c.operation==="interactions.respond");
  assert.equal(approval.input.requestId,"approval-one");assert.equal(approval.input.response.choice,"once");
  assert.deepEqual(calls.filter(c=>c.operation==="interactions.respond"&&c.input.requestId==="clarify-batch").map(c=>c.input.response),[
    {answer:"Release",questionId:"goal"},{answer:"Markdown",questionId:"format"},{answer:"Markdown",questionId:"format"}
  ]);
  assert.equal(calls.filter(c=>c.operation==="conversations.send").length,1);
  assert.equal(errors.length,0,errors.join("\n"));
  console.log("PASS: Agents view, transcript, activity, instructions, approval routing, compact layout (synthetic data).");
}catch(error){const page=await app.firstWindow();console.error((await page.locator("body").innerText()).slice(-5000));await page.screenshot({path:"artifacts/agents-ui-failure.png"});throw error;}finally{await app.close();await fs.rm(profile,{recursive:true,force:true});}
