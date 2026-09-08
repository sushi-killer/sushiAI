import { useEffect, useRef, useState } from "react";
type GitStatus={path:string;repository:boolean;branch?:string;detached?:boolean;ahead?:number;behind?:number;truncated?:boolean;files:{path:string;status:string;added:number;removed:number;staged:boolean}[]};
export function AgentGitPanel({providerId,agentId,onClose}:{providerId:string;agentId:string;onClose:()=>void}){
  const [status,setStatus]=useState<GitStatus|null>(null),[path,setPath]=useState("");
  const [file,setFile]=useState(""),[diff,setDiff]=useState("");
  const [busy,setBusy]=useState(false),[error,setError]=useState("");
  const generation=useRef(0);
  const call=<T,>(action:string,input:Record<string,unknown>)=>window.bridge!.agentCall<T>(providerId,`addons.git.${action}`,{agentId,...input});
  const refresh=async(target?:string)=>{
    const current=++generation.current;setBusy(true);setError("");setFile("");setDiff("");
    try{const result=await call<GitStatus>("status",{path:target});if(current===generation.current){setStatus(result);setPath(result.path);}}
    catch(e){if(current===generation.current){setError(String(e));setStatus(null);}}
    finally{if(current===generation.current)setBusy(false);}
  };
  useEffect(()=>{void refresh();return()=>{generation.current++;};},[providerId,agentId]);
  return <section className="agent-addon-panel agent-git-panel">
    <header><div><h2>Git changes</h2><p className="agent-muted">Review working tree changes against HEAD, including new files.</p></div><button onClick={onClose}>Back to chat</button></header>
    <form className="agent-actions" onSubmit={e=>{e.preventDefault();void refresh(path);}}>
      <input aria-label="Git project path" value={path} onChange={e=>setPath(e.target.value)} placeholder="Absolute project path" />
      <button disabled={busy}>Refresh</button>
    </form>
    {error&&<p role="alert">{error}</p>}{busy&&<p role="status">Loading changes…</p>}
    {status&&!status.repository&&<p>This folder is not in a Git repository.</p>}
    {status?.repository&&<>
      <p className="agent-muted">{status.detached?"Detached HEAD":status.branch||"Unborn branch"} · {status.files.length} changed files · {status.ahead||0} ahead · {status.behind||0} behind</p>
      {status.truncated&&<p>Showing the first 2,000 files.</p>}
      {!status.files.length&&<p>Working tree clean.</p>}
      <div className="agent-files-content"><nav aria-label="Git changed files">
        {status.files.map(entry=><button key={entry.path} disabled={busy} aria-pressed={file===entry.path} onClick={async()=>{
          const current=++generation.current;setBusy(true);setError("");setFile(entry.path);setDiff("");
          try{const result=await call<{diff:string}>("diff",{path:status.path,file:entry.path});if(current===generation.current)setDiff(result.diff);}
          catch(e){if(current===generation.current)setError(String(e));}finally{if(current===generation.current)setBusy(false);}
        }}>{entry.status} {entry.path} <small>+{entry.added} −{entry.removed}{entry.staged?" · staged":""}</small></button>)}
      </nav><div className="agent-git-diff">
        {file&&<strong>{file}</strong>}
        {diff?<pre aria-label="File diff">{diff.split("\n").map((line,index)=><span key={index} className={line.startsWith('+')?'addition':line.startsWith('-')?'deletion':line.startsWith('@@')?'hunk':''}>{line}{"\n"}</span>)}</pre>:<p>{file&&!busy?"No textual diff against HEAD. Binary changes may be shown in Git's summary.":"Select a changed file."}</p>}
      </div></div>
    </>}
  </section>;
}
