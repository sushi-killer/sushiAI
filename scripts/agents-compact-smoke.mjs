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
  await page.getByLabel("Message agent",{exact:true}).waitFor();
  assert.equal(await page.getByRole("navigation",{name:"Agent resources"}).isVisible(),false);
  assert.equal(await page.getByRole("button",{name:"Run a skill",exact:true}).isVisible(),false);
  const height=await page.locator(".agent-composer").evaluate(el=>el.getBoundingClientRect().height);
  assert.ok(height<=125,`Composer height: ${height}`);
  await page.getByLabel("Message agent",{exact:true}).fill("First line\nSecond line\nThird line\nFourth line");
  const expanded=await page.getByLabel("Message agent",{exact:true}).evaluate(el=>el.getBoundingClientRect().height);
  assert.ok(expanded>60);
  await page.getByLabel("Message agent",{exact:true}).fill("");
  await page.getByLabel("Message tools",{exact:true}).click();
  await page.getByRole("button",{name:"Run a skill",exact:true}).waitFor();
  await page.getByLabel("Attach files",{exact:true}).setInputFiles({name:"synthetic.txt",mimeType:"text/plain",buffer:Buffer.from("test")});
  await page.getByRole("button",{name:"Remove synthetic.txt",exact:true}).click();
  await page.getByLabel("Message tools",{exact:true}).click();
  await page.getByRole("button",{name:"Agent settings",exact:true}).click();
  await page.getByRole("button",{name:"Models",exact:true}).waitFor();
  await page.getByText("Conversation settings",{exact:true}).waitFor();
  await page.getByRole("button",{name:"Agent settings",exact:true}).click();
  assert.equal(await page.getByText("Conversation settings",{exact:true}).count(),0);
  await fs.mkdir("artifacts",{recursive:true});
  await page.screenshot({path:"artifacts/agents-compact-composer-desktop.png"});
  await app.evaluate(({BrowserWindow})=>BrowserWindow.getAllWindows()[0].setSize(600,440));
  const layout=await page.locator(".agents-view").evaluate(el=>({width:el.clientWidth,scroll:el.scrollWidth}));
  assert.ok(layout.scroll<=layout.width+1,JSON.stringify(layout));
  await page.screenshot({path:"artifacts/agents-compact-composer-small.png"});
  // Narrow windows hide the shared sidebar, as in Code; the roster is behind the toggle.
  await page.getByLabel("Toggle sidebar",{exact:true}).click();
  assert.equal(await page.locator(".agent-roster-row.selected .agent-face i").count(),2);
  assert.equal(errors.length,0,errors.join("\n"));
  console.log("PASS: compact composer, auto sizing, hidden settings, tools, attachments and narrow layout");
} finally { await app.close(); await fs.rm(profile,{recursive:true,force:true}); }
