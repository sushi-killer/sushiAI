// Synthetic demo only. No host connections, shell commands or user profile access.
module.exports = function installMock() {
 const endpoint='local';
 const leaf=id=>({type:'leaf',id});
 const split=(a,b,axis='row',ratio=.5)=>({type:'split',id:crypto.randomUUID(),a,b,axis,ratio});
 const panels=[
  {id:'demo-claude',sessionId:'s-claude',kind:'agent',title:'Claude Code',agent:'claude',status:'working'},
  {id:'demo-codex',sessionId:'s-codex',kind:'agent',title:'Codex',agent:'codex',status:'idle'},
  {id:'demo-shell',sessionId:'s-shell',kind:'terminal',title:'zsh'},
  {id:'demo-chat',kind:'chat',title:'Thread',agent:'claude',messages:[{id:'q',role:'user',text:'Make the checkout feel effortless.'},{id:'a',role:'assistant',text:'Added a faster checkout flow, clearer errors, and a little motion. Ready for your review.'}]}
 ];
 const ws={id:'demo-orbit',name:'orbit',connection:endpoint,cwd:'/projects/orbit',panels,layout:split(leaf('demo-claude'),split(leaf('demo-codex'),split(leaf('demo-shell'),leaf('demo-chat'),'column',.42),'column',.42),'row',.54)};
 const snapshot=JSON.stringify({workspaces:[ws],activeId:ws.id,socket:endpoint,routines:[],fontScale:1});
 const terminalText={
  'demo-claude': '\x1b[38;2;214;140;107m  ✳ Claude Code\x1b[0m\r\n\x1b[90m  orbit / checkout\x1b[0m\r\n\r\n\x1b[38;2;133;171;255m❯\x1b[0m Build a checkout people love.\r\n  Keep it fast. Make it feel effortless.\r\n\r\n\x1b[38;2;226;197;119m●\x1b[0m Read  src/checkout/Checkout.tsx\r\n\x1b[38;2;226;197;119m●\x1b[0m Edit  src/checkout/Checkout.tsx\r\n\x1b[38;2;125;217;164m  +42\x1b[0m  \x1b[38;2;222;127;132m−18\x1b[0m\r\n\x1b[38;2;226;197;119m●\x1b[0m Edit  src/styles/checkout.css\r\n\x1b[38;2;125;217;164m  +28\x1b[0m  \x1b[38;2;222;127;132m−6\x1b[0m\r\n\x1b[38;2;226;197;119m●\x1b[0m Run   npm test -- checkout\r\n\r\n\x1b[38;2;125;217;164m  ✓ 24 tests passed\x1b[0m\r\n  \x1b[90m0 failed · 1.2s\x1b[0m\r\n\r\n  A simpler checkout. Instant validation.\r\n  A little less friction, a lot more flow.\r\n\r\n\x1b[38;2;133;171;255m❯\x1b[0m ',
  'demo-codex': '\x1b[1m  OpenAI Codex\x1b[0m\r\n\x1b[90m  orbit / accessibility\x1b[0m\r\n\r\n› Review the new checkout flow.\r\n\r\n\x1b[38;2;125;217;164m✓\x1b[0m Keyboard navigation\r\n\x1b[38;2;125;217;164m✓\x1b[0m Focus states and labels\r\n\x1b[38;2;125;217;164m✓\x1b[0m Reduced-motion support\r\n\r\n  Looks good. Ready to ship.\r\n',
  'demo-shell': '\x1b[38;2;125;217;164m❯\x1b[0m npm run dev\r\n\r\n  \x1b[38;2;167;147;244mVITE\x1b[0m  ready in \x1b[1m184 ms\x1b[0m\r\n\r\n  \x1b[38;2;125;217;164m➜\x1b[0m Local:  http://localhost:3000/\r\n'
 };
 const terminalListeners=new Set();
 const bridge={
  // No data folder in the browser: the fake bridge serves the seeded snapshot and drops writes.
  workspaceStateRead:()=>snapshot,workspaceStateWrite:async()=>{},workspaceStateFlush:()=>{},
  system:async()=>({home:'/projects',cwd:'/projects/orbit',platform:'darwin',socketPath:'/demo/sushiai.sock',agents:['claude','codex','gemini','cursor-agent'].map(name=>({name,path:'/demo/bin/'+name}))}),
  // The daemon: one ready host, one session per terminal-like panel, and a screen for each.
  daemonStates:async()=>[{host:'local',state:'ready',generation:1,version:'0.1.0',capabilities:['sessions','attach','resync','agents','catalog']}],
  onDaemonState:()=>()=>{},onDaemonEvent:()=>()=>{},
  sessionsList:async()=>panels.filter(p=>p.sessionId).map(p=>({id:p.sessionId,cmd:[p.agent||'zsh'],cwd:ws.cwd,status:'running',cols:120,rows:32,...(p.agent?{agent:p.agent,agentStatus:p.status||'idle',statusSource:'hook'}:{})})),
  daemonTerminalAttach:async({panelId})=>{
   setTimeout(()=>terminalListeners.forEach(l=>l({panelId,snapshot:terminalText[panelId]||'',cols:120,rows:32})),30);
  },
  onDaemonTerminal:cb=>{terminalListeners.add(cb);return()=>terminalListeners.delete(cb)},
  daemonTerminalWrite:async()=>{},daemonTerminalResize:async()=>{},daemonTerminalDetach:async()=>{},daemonTerminalAck:async()=>{},
  extensionsList:async()=>({schemaVersion:2,version:0,problems:[],extensions:[],surfaces:[],navigation:[],actions:[],commands:[]}),
  onChat:()=>()=>{},catalog:async()=>[],chooseDirectory:async()=>'/projects/orbit',window:async()=>{},
  connectionsList:async()=>[{id:'demo-remote',host:'studio',name:'Studio server',connected:true}],
  projectInspect:async(_e,o)=>o.operation==='git'?{branch:'feat/checkout',changes:[{path:'src/checkout/Checkout.tsx',status:' M'},{path:'src/styles/checkout.css',status:' M'},{path:'tests/checkout.test.ts',status:'??'}]}:o.operation==='diff'?{text:'diff --git a/src/checkout/Checkout.tsx b/src/checkout/Checkout.tsx\nindex e8a0100..fc51200 100644\n--- a/src/checkout/Checkout.tsx\n+++ b/src/checkout/Checkout.tsx\n@@ -18,8 +18,14 @@ export function Checkout() {\n   return (\n-    <form onSubmit={submit}>\n-      <input placeholder="Email" />\n-      <button>Pay</button>\n+    <form onSubmit={submit} aria-label="Checkout">\n+      <EmailField\n+        label="Your email"\n+        validateOn="blur"\n+        autoComplete="email"\n+      />\n+      <PayButton loading={pending}>\n+        Complete your order\n+      </PayButton>\n     </form>\n   );\n }'}:{entries:[{name:'src',path:'src',directory:true},{name:'tests',path:'tests',directory:true},{name:'README.md',path:'README.md',directory:false},{name:'index.html',path:'index.html',directory:false}]},
 };
 // Anything the demo does not draw is a no-op: subscriptions return an unsubscribe.
 window.bridge=new Proxy(bridge,{get:(target,key)=>key in target?target[key]:/^on[A-Z]/.test(String(key))?()=>()=>{}:async()=>undefined});
};