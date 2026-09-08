import { useEffect, useRef, useState } from "react";
type Listing={path:string;parent:string;entries:{name:string;path:string;isDirectory:boolean}[];truncated:boolean};
type FileView={path:string;text:string;revision:string;binary:boolean;truncated:boolean;byteSize:number;readOnlyReason?:string;imageData?:string};
const drafts = new Map<string,{listing:Listing|null;file:FileView|null;draft:string;location:string}>();
export function AgentFilesPanel({providerId,agentId,onClose}:{providerId:string;agentId:string;onClose:()=>void}) {
  const key=JSON.stringify([providerId,agentId]);
  const previous=drafts.get(key);
  const [listing,setListing]=useState<Listing|null>(previous?.listing||null),[location,setLocation]=useState(previous?.location||"");
  const [file,setFile]=useState<FileView|null>(previous?.file||null),[draft,setDraft]=useState(previous?.draft||"");
  useEffect(()=>{drafts.set(key,{listing,file,draft,location});},[key,listing,file,draft,location]);
  const [busy,setBusy]=useState(false),[error,setError]=useState(""),[notice,setNotice]=useState("");
  const generation=useRef(0);
  const dirty=file && draft!==file.text;
  const call=<T,>(action:string,input:Record<string,unknown>={})=>window.bridge!.agentCall<T>(providerId,`addons.files.${action}`,{agentId,...input});
  const load=async(path?:string,isDirectory=true)=>{
    const current=++generation.current;
    setBusy(true);setError("");setNotice("");
    try {
      if(isDirectory){const result=await call<Listing>("list",{path});if(current===generation.current){setListing(result);setLocation(result.path);setFile(null);setDraft("");}}
      else {const result=await call<FileView>("read",{path});if(current===generation.current){setFile(result);setDraft(result.text);}}
    } catch(e){if(current===generation.current)setError(String(e));}
    finally{if(current===generation.current)setBusy(false);}
  };
  useEffect(()=>{if(!previous?.listing)void load();return()=>{generation.current++;};},[providerId,agentId]);
  return <section className="agent-addon-panel agent-files-panel">
    <header><div><h2>Files</h2><p className="agent-muted">Browse the agent's project and edit text files.</p></div><button disabled={Boolean(dirty)||busy} onClick={onClose}>Back to chat</button></header>
    <form className="agent-actions" onSubmit={event=>{event.preventDefault();void load(location);}}>
      <input aria-label="Agent folder path" value={location} onChange={event=>setLocation(event.target.value)} placeholder="Absolute folder path" />
      <button disabled={busy||Boolean(dirty)}>Open folder</button>
      <button type="button" disabled={busy||Boolean(dirty)||!listing||listing.path===listing.parent} onClick={()=>void load(listing!.parent)}>Up</button>
    </form>
    {error&&<p role="alert">{error}</p>}{notice&&<p role="status">{notice}</p>}
    {busy&&<p role="status">Loading…</p>}
    {dirty&&<p className="agent-muted">Unsaved changes. Save or discard before opening another file.</p>}
    <div className="agent-files-content"><nav aria-label="Agent files">
      {listing?.truncated&&<p>Showing the first 5,000 entries.</p>}
      {listing?.entries.map(entry=><button key={entry.path} disabled={busy||Boolean(dirty)} onClick={()=>void load(entry.path,entry.isDirectory)}>{entry.isDirectory?"▸ ":""}{entry.name}</button>)}
      {listing&&!listing.entries.length&&<p>Empty folder</p>}
    </nav><div className="agent-file-editor">
      {file?<><strong>{file.path}</strong><small>{file.byteSize.toLocaleString()} bytes</small>
        {file.imageData?<figure className="agent-image"><img src={file.imageData} alt={file.path.split('/').pop() || 'File preview'} onError={()=>{setError('The image could not be decoded.');setFile({...file,imageData:undefined});}} /></figure>:file.binary?<p>Binary file. A text preview is unavailable.</p>:<>
          {file.truncated&&<p>Partial preview. Editing is disabled.</p>}
          {file.readOnlyReason&&<p role="status">{file.readOnlyReason}</p>}
          <textarea aria-label="File contents" spellCheck={false} readOnly={busy||file.truncated||Boolean(file.readOnlyReason)} value={draft} onChange={event=>setDraft(event.target.value)} />
          <div className="agent-actions"><button disabled={busy||file.truncated||Boolean(file.readOnlyReason)||!dirty} onClick={async()=>{
            setBusy(true);setError("");setNotice("");
            try{const saved=await call<FileView>("save",{path:file.path,revision:file.revision,content:draft});setFile(saved);setDraft(saved.text);setNotice("File saved.");}
            catch(e){setError(String(e));}finally{setBusy(false);}
          }}>Save file</button><button disabled={busy||!dirty} onClick={()=>{setDraft(file.text);setNotice("");}}>Discard changes</button></div>
        </>}
      </>:<p>Select a file to preview it.</p>}
    </div></div>
  </section>;
}
