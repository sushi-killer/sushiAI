const {text}=require('./registry.cjs');
function installHermesCommands(provider){
  provider.descriptor.capabilities.push('skill-commands');
  const catalog=async session=>{
    const result=await provider.transport(session.agentId).rpc(session.agentId,'commands.catalog',{session_id:session.runtime});
    if(!Array.isArray(result.pairs)||!result.skills||typeof result.skills!=='object')throw Error('Hermes command catalog is unavailable.');
    return {commands:result.pairs.filter(pair=>Array.isArray(pair)&&typeof pair[0]==='string'&&Object.hasOwn(result.skills,pair[0]))
      .slice(0,2000).map(([name,description])=>({name,description:String(description||'').slice(0,1000)})),warning:String(result.warning||'').slice(0,1000)};
  };
  provider.operations.set('conversations.skills',input=>catalog(provider.get(input)));
  provider.operations.set('conversations.runSkill',async input=>{
    if(Object.keys(input).some(key=>!['agentId','conversationId','command','arguments'].includes(key)))throw Error('Unknown skill invocation field.');
    const session=provider.get(input),command=text(input.command,'skill command',240);
    const args=input.arguments??'';
    if(typeof args!=='string'||args.length>100000||args.includes('\0'))throw Error('Invalid skill arguments.');
    if(session.settingsPending||session.submitting||['running','working','waiting','sending'].includes(session.state.status))throw Error('Wait for this turn to finish before running a skill.');
    session.submitting=true;
    let delegated=false;
    try{
      const available=await catalog(session);
      if(!available.commands.some(item=>item.name===command))throw Error('This skill is not available in the selected agent profile.');
      const result=await provider.transport(session.agentId).rpc(session.agentId,'command.dispatch',{
        session_id:session.runtime,name:command.replace(/^\//,''),arg:args,
      });
      if(result.type!=='skill'||typeof result.message!=='string'||!result.message.trim())throw Error('Hermes did not return a skill invocation.');
      session.submitting=false;
      const sending=provider.send({agentId:session.agentId,conversationId:session.id,text:result.message},{displayText:`${command}${args?' '+args:''}`});
      delegated=true;
      return await sending;
    }finally{if(!delegated)session.submitting=false;}
  });
}
module.exports={installHermesCommands};
