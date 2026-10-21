// Material provider: Pi proves received bodies; Rust supplies fixed Git objects.
import {createHash} from 'node:crypto';
import {renderBridgePart} from './context-bridge.mjs';
const sha=value=>createHash('sha256').update(value).digest('hex');
const text=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
const frameValue=(entry,frame)=>{
  const value=entry.content.slice(frame.start,frame.end);
  if(!Number.isSafeInteger(frame.start)||!Number.isSafeInteger(frame.end)||frame.start<0||frame.end<frame.start||frame.end>entry.content.length||sha(value)!==(frame.kind==='full'?frame.textHash:frame.patchHash))throw Error('CONTEXT_CONFLICT');
  return {...frame,...(frame.kind==='full'?{text:value}:{unifiedDiff:value})};
};
export function receivedMaterialFrames(entry) {
  if(entry.details?.materialFrames)return entry.details.materialFrames.map(f=>frameValue(entry,f));
  return JSON.parse(entry.content).frames??[]; // Previously accepted native JSON projections.
}
function viewport(f) {
  const groups=[];
  for(const p of f.paragraphs??[]){
    if(Number.isSafeInteger(p.startChar)&&Number.isSafeInteger(p.endChar))groups.push('['+p.startChar+', '+p.endChar+')（Unicode 字符）');
    else if(Number.isSafeInteger(p.start)&&Number.isSafeInteger(p.end))groups.push('['+p.start+', '+p.end+')（UTF-16）');
  }
  return groups.length?groups.join('、')+'；从 0 开始，左闭右开':'本次未捕获';
}
function focusLocation(f,path) {
  let value='文件：'+path+'\n版本：'+(f.commitHash??'未绑定 Git 版本')+'\n可见范围：'+viewport(f);
  if(f.selectionRanges?.length)value+='\n用户选区：'+f.selectionRanges.map(r=>'['+r.startChar+', '+r.endChar+')').join('、')+'（正文 Unicode 字符，从 0 开始，左闭右开）';
  if(f.exercise)value+='\n当前题目：第 '+f.exercise.number+' 题';
  return value;
}
export async function registerMaterialContext({manager,spec,loadChanges,envelope,bridge,containsSecret,apiKey,NativeSessionError}) {
  const materialBaseline=id=>{
    const candidates=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.details?.sourceId===id&&(e.sourceEntry.customType==='learning-station-material'||e.sourceEntry.details?.materialFrames));
    const active=candidates.filter(e=>manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-material-request'&&r.data?.entryId===e.sourceEntry.id&&r.data.nativeSessionId===manager.getSessionId()));
    for(const e of candidates)if(!active.includes(e))bridge.exclude(e.sourceEntry.id);
    let activeHash,chainReady=false;
    for(const entry of active){
      let frames;try{frames=receivedMaterialFrames(entry.sourceEntry);}catch{chainReady=false;break;}
      for(const frame of frames){
        if(frame.kind==='full'){chainReady=sha(frame.text)===frame.textHash;activeHash=frame.textHash;}
        else if(frame.kind==='diff'){chainReady=chainReady&&frame.baseHash===activeHash;activeHash=frame.textHash;}
        else chainReady=false;
      }
    }
    return {active,last:chainReady?active.at(-1)?.sourceEntry.details:undefined};
  };
  let currentSources=envelope.nativeMaterials??[],courseChanges,focusPath=envelope.focus?.path;
  const inventory=structuredClone(envelope.contextBridgeManifest??{version:1,entries:[]});
  const metadataOnly=envelope.metadataOnlySourceIds??[];
  if(!Array.isArray(metadataOnly)||metadataOnly.some(id=>!spec.authorizedSourceIds.includes(id)))throw new NativeSessionError();
  for(const entry of manager.buildSessionProjection().entries)if(entry.messages.length&&entry.sourceEntry.type==='custom_message'&&metadataOnly.includes(entry.sourceEntry.details?.sourceId)&&(entry.sourceEntry.customType==='learning-station-material'||entry.sourceEntry.details?.materialFrames))bridge.exclude(entry.sourceEntry.id);
  inventory.entries=inventory.entries.filter(e=>e.kind!=='history');
  if(loadChanges&&envelope.focus){
    const focus=envelope.focus;
    if(focus.asset&&!spec.authorizedSourceIds.includes(focus.asset.id))throw new NativeSessionError();
    const received=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.details?.courseId===focus.courseId&&e.sourceEntry.details?.checkpointCommitOid).map(e=>e.sourceEntry).filter(e=>manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-changes-request'&&r.data?.changesEntryId===e.id&&r.data.runId===e.details.runId&&r.data.userEntryId===e.details.userEntryId&&r.data.nativeSessionId===manager.getSessionId())&&manager.getEntries().some(u=>u.type==='custom'&&u.customType==='learning-station-user'&&u.data?.userEntryId===e.details.userEntryId&&manager.getEntry(u.data.nativeId)?.message?.role==='user')).at(-1);
    const body=materialBaseline(focus.asset?.id??focus.documentId).last;
    const focusBaseCommitOid=body?.range?.complete&&body.path===focus.path?body.gitCommitOid??null:null;
    const changes=await loadChanges({baseCommitOid:received?.details.checkpointCommitOid??null,focusBaseCommitOid});
    if(changes?.complete!==true||changes.checkpoint?.courseId!==focus.courseId||changes.checkpoint.checkpointCommitOid!==focus.commitHash||changes.checkpoint.materialTreeOid!==focus.materialTreeOid||changes.baseCommitOid!==(received?.details.checkpointCommitOid??null)||!Array.isArray(changes.changes)||typeof changes.stat!=='string'||typeof changes.focusPath!=='string')throw new NativeSessionError();
    if(Buffer.byteLength(JSON.stringify(changes))>512*1024)throw new NativeSessionError('LIMIT_EXCEEDED');
    if(containsSecret(changes,apiKey))throw new NativeSessionError('AUTH');
    if((changes.focused?.textUnavailable||focus.binary||['image','pdf','exercise'].includes(focus.type))&&(typeof changes.focused?.text==='string'||focus.paragraphs?.some(p=>p.text?.length)))throw new NativeSessionError('CONTEXT_CONFLICT');
    currentSources=typeof changes.focused?.text==='string'?[changes.focused]:[];
    for(const source of currentSources)if(sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
    if(currentSources.length+changes.changes.length+(changes.checkpoint.excludedFiles?.length??0)+inventory.entries.filter(e=>e.kind!=='material').length>256)throw new NativeSessionError('LIMIT_EXCEEDED');
    inventory.entries=inventory.entries.filter(e=>e.kind!=='material');
    for(const source of currentSources)inventory.entries.push({id:source.id,kind:'material',sourceId:source.id,path:source.path,version:source.version,includedChars:[...source.text].length,range:source.range,bodyStartChar:0,coordinateSpace:'body'});
    if(!currentSources.length&&!changes.focused?.textUnavailable)inventory.entries.push({id:'focus:'+(focus.asset?.id??focus.documentId),kind:'material',status:'added',sourceId:focus.asset?.id??focus.documentId,path:focus.path,version:focus.documentVersion,includedChars:0,reason:'focus_metadata_only'});
    if(changes.focused?.textUnavailable)inventory.entries.push({id:changes.focused.id,kind:'material',status:'retained',sourceId:changes.focused.id,version:focus.documentVersion??focus.commitHash,includedChars:0,reason:'text_unavailable:'+changes.focused.textUnavailable,fixedVersion:{courseId:focus.courseId,checkpointCommitOid:focus.commitHash,path:changes.focused.path,blobOid:changes.focused.gitBlobOid}});
    for(const change of changes.changes)if(change.path!==focus.path)inventory.entries.push({id:'change:'+change.path,kind:'material',status:change.status==='A'?'added':'updated',version:focus.commitHash,includedChars:0,path:change.path,fileStatus:change.status,added:change.added,deleted:change.deleted,binary:change.binary,fixedVersion:change.fixedVersion,reason:'git_stat'});
    for(const [index,excluded] of (changes.checkpoint.excludedFiles??[]).entries())inventory.entries.push({id:'excluded:'+index+':'+excluded.assetId,kind:'material',status:'retained',...(excluded.assetId?{sourceId:excluded.assetId}:{}),version:focus.commitHash,includedChars:0,reason:'excluded:'+excluded.reason,fileName:excluded.fileName});
    courseChanges=changes;focusPath=changes.focusPath;
  }
  if(envelope.focus){
    const f=envelope.focus;
    bridge.message('focus','【Focus：用户当前阅读的位置】','learning-station-focus',focusLocation(f,focusPath),{courseId:f.courseId,checkpointCommitOid:f.commitHash});
  }
  let patchSource,patchBaseline;
  for(const source of currentSources){
    if(source.projectionError)throw new NativeSessionError('CONTEXT_CONFLICT');
    if(typeof source.text!=='string'||sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
    const {last}=materialBaseline(source.id);
    const ever=manager.getEntries().some(e=>e.type==='custom_message'&&e.details?.sourceId===source.id);
    let status=ever?'reloaded':'added';
    const unchanged=last&&source.range?.complete&&last.range?.complete&&(!source.path||last.path===source.path)&&last.textHash===source.textHash;
    const diff=!unchanged&&courseChanges&&source.range?.complete&&last?.range?.complete&&last.path===source.path&&source.gitChange?.unifiedDiff&&last.gitCommitOid===source.gitChange.baseCommitOid&&last.gitBlobOid===source.gitChange.baseBlobOid;
    if(unchanged)status='retained';
    else if(diff){patchSource=source;patchBaseline=last;status='updated';}
    else {
      const description=source.range?.complete===false?'以下为已提供范围的正文（部分内容）':source.gitCommitOid?'以下为文件全文（固定 Git 版本；'+(ever?'正文基准已丢失，重新载入':'首次正文基准')+'）':'以下为笔记全文（未绑定 Git 版本）';
      const prefix=envelope.focus?'':('文件：'+(source.modelPath??source.path??'未记录路径')+'\n');
      const value=prefix+source.text,start=renderBridgePart(description,prefix).length;
      bridge.message('focus',description,'learning-station-material',value,{sourceId:source.id,path:source.path,textHash:source.textHash,version:source.version,range:source.range,...(source.gitCommitOid?{gitCommitOid:source.gitCommitOid,gitBlobOid:source.gitBlobOid}:{}),materialFrames:[{kind:'full',textHash:source.textHash,start,end:start+source.text.length}]});
    }
    const item=inventory.entries.find(e=>e.sourceId===source.id&&e.kind==='material');
    if(item){item.includedChars=[...source.text].length;item.range=source.range;item.bodyStartChar=0;item.coordinateSpace='body';item.status=status;item.reason=status==='retained'?'native_projection_contains_baseline':status==='updated'?'native_projection_verified_diff':'native_baseline_reload';}
  }
  if(courseChanges&&(courseChanges.stat||patchSource)){
    const description='【Changes：课程材料相对已知版本的变化】',f=envelope.focus;
    let value=courseChanges.baseCommitOid?'比较版本：'+courseChanges.baseCommitOid+' → '+f.commitHash:'首次建立课程材料基准：'+f.commitHash+'（以下为 Git 空树比较的原生 stat）';
    value+='\n\n'+(courseChanges.stat||'课程材料相对已知版本无变化。\n');
    const details={courseId:f.courseId,checkpointCommitOid:f.commitHash};
    if(patchSource){
      const patch=patchSource.gitChange.unifiedDiff;
      value+='\n当前聚焦文件的完整 Git diff（正文基准 '+patchBaseline.gitCommitOid+' → '+f.commitHash+'）：\n';
      const start=renderBridgePart(description,value).length;
      Object.assign(details,{sourceId:patchSource.id,path:patchSource.path,textHash:patchSource.textHash,version:patchSource.version,range:patchSource.range,gitCommitOid:patchSource.gitCommitOid,gitBlobOid:patchSource.gitBlobOid,materialFrames:[{kind:'diff',baseHash:patchBaseline.textHash,textHash:patchSource.textHash,patchHash:sha(patch),start,end:start+patch.length}]});
      value+=patch;
    }
    bridge.message('material_changes',description,'learning-station-changes',value,details);
  }
  for(const item of inventory.entries)if(item.kind==='turn'&&(item.id==='focus'||item.id.startsWith('focus:'))&&currentSources.length){item.includedChars=0;item.reason='focus_location';}
  return inventory;
}
