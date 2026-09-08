const { text } = require('./registry.cjs');

function splitImageReferences(value) {
  if (typeof value !== 'string') return [];
  const result = [];
  const pattern = /^@image:(?:`([^`\n]+)`|"([^"\n]+)"|'([^'\n]+)'|([^\s]+))[\t ]*$/gm;
  let offset = 0;
  for (const match of value.matchAll(pattern)) {
    const path = match[1] || match[2] || match[3] || match[4];
    if (path.length > 4096 || !path.startsWith('/') || /[\x00-\x1f]/.test(path)) continue;
    if (match.index > offset) result.push({kind:'text',text:value.slice(offset,match.index)});
    result.push({kind:'image',path,name:path.split('/').pop()});
    offset=match.index+match[0].length;
  }
  if (offset<value.length) result.push({kind:'text',text:value.slice(offset)});
  return result;
}

function validateImageData(value) {
  if (typeof value !== 'string' || value.length > 2*1024*1024) throw Error('Image is too large to preview.');
  const match=/^data:image\/(png|jpeg|gif|webp|bmp);base64,([A-Za-z0-9+/]+={0,2})$/.exec(value);
  if (!match) throw Error('Unsupported image response.');
  const bytes=Buffer.from(match[2],'base64');
  if (!bytes.length || bytes.toString('base64')!==match[2]) throw Error('Invalid image response.');
  return value;
}
function installHermesMedia(provider) {
  provider.descriptor.capabilities.push('image-preview');
  provider.operations.set('conversations.media', async(input)=>{
    if(Object.keys(input).some(key=>!['agentId','conversationId','itemId'].includes(key)))
      throw Error('Unknown image request field.');
    const session=provider.get(input), id=text(input.itemId,'image item ID',1000);
    const item=session.state.items.find(item=>item.id===id && item.kind==='image');
    if (!item || typeof item.path!=='string') throw Error('Image is not present in this conversation.');
    const result=await provider.transport(session.agentId).request('GET','/api/media',{
      profile:session.agentId,query:{path:item.path},
    });
    return {dataUrl:validateImageData(result?.data_url),name:item.name || 'Attached image'};
  });
}
module.exports={splitImageReferences,validateImageData,installHermesMedia};
