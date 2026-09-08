const { createHash } = require('node:crypto');
const path = require('node:path');
const { text } = require('./registry.cjs');
const { validateImageData } = require('./hermes-media.cjs');
const revision = (agent, file) => createHash('sha256').update(JSON.stringify([agent,file.path,file.text,file.byteSize])).digest('hex');
function installHermesFiles(provider) {
  provider.descriptor.addons.push({id:'files',name:'Files',description:'Browse and edit agent project files'});
  const pending=new Set();
  for (const action of ['list','read','save']) provider.operations.set(`addons.files.${action}`,async(input)=>{
    const allowed=['agentId','path',...(action==='save'?['content','revision']:[])];
    if(Object.keys(input).some(key=>!allowed.includes(key)))throw Error('Unknown file request field.');
    const agent=await provider.agent(input);
    const request=(method,route,options={})=>provider.transport(agent).request(method,`/api/fs/${route}`,{profile:agent,...options});
    let target=input.path;
    if(action==='list' && !target)target=(await request('GET','default-cwd')).cwd;
    target=text(target,'file path',4096);
    if(!path.isAbsolute(target)||/[\r\n]/.test(target))throw Error('Enter an absolute file path.');
    const read=async()=>{
      if (/\.(png|jpe?g|gif|webp|bmp)$/i.test(target)) {
        const raw = await request('GET','read-data-url',{query:{path:target}});
        const imageData = validateImageData(raw?.dataUrl);
        return {path:target,text:'',revision:'',binary:true,truncated:false,
          byteSize:Buffer.from(imageData.slice(imageData.indexOf(',')+1),'base64').length,imageData};
      }
      const data=await request('GET','read-text',{query:{path:target}});
      if(typeof data?.text!=='string'||typeof data.path!=='string')throw Error('Invalid file response.');
      let readOnlyReason = '';
      // Hermes decodes previews with errors="replace". Verify suspect text
      // against the native byte endpoint before allowing a UTF-8 overwrite.
      if (!data.binary && !data.truncated && data.text.includes('\uFFFD')) {
        try {
          const raw = await request('GET','read-data-url',{query:{path:target}});
          const match = /^data:[^,]*;base64,([A-Za-z0-9+/]*={0,2})$/.exec(raw?.dataUrl || '');
          const bytes = match && Buffer.from(match[1], 'base64');
          if (!bytes || bytes.toString('base64') !== match[1] || !bytes.equals(Buffer.from(data.text, 'utf8')))
            readOnlyReason = 'This file is not lossless UTF-8. Editing is disabled to preserve its original bytes.';
        } catch {
          readOnlyReason = 'The original encoding could not be verified. Editing is disabled.';
        }
      }
      return {...data,readOnlyReason,revision:revision(agent,data)};
    };
    if(action==='list'){
      const data=await request('GET','list',{query:{path:target}});
      if(data.error)throw Error(`Directory unavailable: ${data.error}`);
      if(!Array.isArray(data.entries))throw Error('Invalid directory response.');
      return {path:target,parent:path.dirname(target),entries:data.entries.slice(0,5000),truncated:data.entries.length>5000};
    }
    if(action==='read')return read();
    if(typeof input.content!=='string'||Buffer.byteLength(input.content)>512*1024)throw Error('Text must be no larger than 512 KB.');
    text(input.revision,'file revision',128);
    if(pending.has(agent))throw Error('Wait for the current file save to finish.');
    pending.add(agent);
    try{
      const current=await read();
      if(current.binary||current.truncated)throw Error('Only complete text files can be edited.');
      if(current.readOnlyReason)throw Error(current.readOnlyReason);
      if(current.revision!==input.revision)throw Error('File changed on disk. Reopen it before saving.');
      const result=await request('POST','write-text',{body:{path:target,content:input.content}});
      if(result?.ok!==true)throw Error('Hermes did not confirm the file save.');
      const saved=await read();
      if(saved.text!==input.content)throw Error('File changed during saving. Reopen it to review the current content.');
      return saved;
    }finally{pending.delete(agent);}
  });
}
module.exports={installHermesFiles};
