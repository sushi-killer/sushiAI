import { useEffect, useState } from "react";
export function AgentSkillPicker({providerId,agentId,conversationId,disabled,onRun}:{providerId:string;agentId:string;conversationId:string;disabled:boolean;onRun:(command:string,args:string)=>Promise<void>}){
  const [open,setOpen]=useState(false),[commands,setCommands]=useState<{name:string;description:string}[]>([]);
  const [search,setSearch]=useState(''),[selected,setSelected]=useState(''),[args,setArgs]=useState('');
  const [loading,setLoading]=useState(false),[error,setError]=useState(''),[warning,setWarning]=useState('');
  useEffect(()=>{
    if(!open)return;
    let disposed=false;setLoading(true);setError('');
    void window.bridge!.agentCall<{commands:{name:string;description:string}[];warning:string}>(providerId,'conversations.skills',{agentId,conversationId})
      .then(result=>{if(!disposed){setCommands(result.commands);setWarning(result.warning);}})
      .catch(e=>{if(!disposed)setError(String(e));})
      .finally(()=>{if(!disposed)setLoading(false);});
    return()=>{disposed=true;};
  },[open,providerId,agentId,conversationId]);
  return <div className="agent-skill-picker">
    <button type="button" disabled={disabled} aria-expanded={open} onClick={()=>setOpen(!open)}>Run a skill</button>
    {open&&<section aria-label="Skill commands">
      <input aria-label="Search skill commands" placeholder="Find a skill…" value={search} onChange={e=>setSearch(e.target.value)}/>
      {loading&&<p role="status">Loading skills…</p>}{error&&<p role="alert">{error}</p>}{warning&&<p>{warning}</p>}
      <div className="agent-skill-options">{commands.filter(item=>`${item.name} ${item.description}`.toLowerCase().includes(search.toLowerCase())).map(item=><button type="button" key={item.name} disabled={disabled||loading} aria-pressed={selected===item.name} onClick={()=>setSelected(item.name)}><strong>{item.name}</strong><small>{item.description}</small></button>)}</div>
      {!loading&&!error&&!commands.length&&<p>No skill commands are available for this agent.</p>}
      {selected&&<><label>Instructions for {selected}<textarea aria-label="Skill instructions" value={args} onChange={e=>setArgs(e.target.value)} disabled={disabled||loading}/></label>
        <button type="button" disabled={disabled||loading} onClick={async()=>{setLoading(true);setError('');try{await onRun(selected,args);setOpen(false);setArgs('');}catch(e){setError(String(e));}finally{setLoading(false);}}}>Run selected skill</button></>}
    </section>}
  </div>;
}
