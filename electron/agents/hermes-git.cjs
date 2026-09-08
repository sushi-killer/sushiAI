const path=require('node:path');
const {text}=require('./registry.cjs');
function installHermesGit(provider){
  provider.descriptor.addons.push({id:'git',name:'Git',description:'Review project changes'});
  for(const action of ['status','diff'])provider.operations.set(`addons.git.${action}`,async input=>{
    if(Object.keys(input).some(key=>!['agentId','path',...(action==='diff'?['file']:[])].includes(key)))throw Error('Unknown Git request field.');
    const agent=await provider.agent(input);
    const request=(route,query)=>provider.transport(agent).request('GET',route,{profile:agent,query});
    const target=text(input.path || (await request('/api/fs/default-cwd')).cwd,'project path',4096);
    if(!path.isAbsolute(target))throw Error('Enter an absolute project path.');
    const root=(await request('/api/fs/git-root',{path:target})).root;
    if(!root){if(action==='status')return {path:target,repository:false,files:[]};throw Error('This folder is not in a Git repository.');}
    if(action==='status'){
      const status=await request('/api/git/status',{path:root});
      if(!status)throw Error('Could not read Git status.');
      const review=await request('/api/git/review/list',{path:root,scope:'uncommitted'});
      if(!Array.isArray(review.files))throw Error('Invalid Git file list.');
      return {path:root,repository:true,branch:status.branch,detached:status.detached,ahead:status.ahead,behind:status.behind,
        files:review.files.slice(0,2000),truncated:review.files.length>2000};
    }
    const file=text(input.file,'Git file',4096);
    if(path.isAbsolute(file)||file.split(/[\\/]/).includes('..')||file.includes('\\'))throw Error('Choose a repository-relative file.');
    const result=await request('/api/git/file-diff',{path:root,file});
    if(typeof result.diff!=='string')throw Error('Invalid Git diff response.');
    return {path:root,file,diff:result.diff};
  });
}
module.exports={installHermesGit};
