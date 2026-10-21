// Material business provider: verifies real Pi baselines and fixed Git objects.
import {createHash} from 'node:crypto';
const sha=value=>createHash('sha256').update(value).digest('hex');
const text=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
export async function registerMaterialContext({manager,spec,loadChanges,envelope,bridge,containsSecret,apiKey,NativeSessionError}) {
  const materialBaseline=id=>{
      const candidates=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.customType==='learning-station-material'&&e.sourceEntry.details?.sourceId===id);
      const active=candidates.filter(e=>manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-material-request'&&r.data?.entryId===e.sourceEntry.id&&r.data.nativeSessionId===manager.getSessionId()));
      for(const e of candidates)if(!active.includes(e))bridge.exclude(e.sourceEntry.id);
      let activeHash;let chainReady=false;
      for(const entry of active){
        let value;try{value=JSON.parse(text(entry.messages[0]));}catch{chainReady=false;break;}
        for(const frame of value.frames??[]){if(frame.kind==='full'){chainReady=sha(frame.text)===frame.textHash;activeHash=frame.textHash;}
        else if(frame.kind==='diff'){chainReady=chainReady&&frame.baseHash===activeHash;activeHash=frame.textHash;}else chainReady=false;}
      }
      return {active,last:chainReady?active.at(-1)?.sourceEntry.details:undefined};
    };
      let currentSources=envelope.nativeMaterials??[];
      const inventory=structuredClone(envelope.contextBridgeManifest??{version:1,entries:[]});
      const metadataOnly=envelope.metadataOnlySourceIds??[];
      if(!Array.isArray(metadataOnly)||metadataOnly.some(id=>!spec.authorizedSourceIds.includes(id)))throw new NativeSessionError();
      for(const entry of manager.buildSessionProjection().entries)if(entry.messages.length&&entry.sourceEntry.type==='custom_message'&&entry.sourceEntry.customType==='learning-station-material'&&metadataOnly.includes(entry.sourceEntry.details?.sourceId))bridge.exclude(entry.sourceEntry.id);
      inventory.entries=inventory.entries.filter(e=>e.kind!=='history');
      if(loadChanges&&envelope.focus){
        const focus=envelope.focus;
        if(focus.asset&&!spec.authorizedSourceIds.includes(focus.asset.id))throw new NativeSessionError();
        // A Git commit alone is never an accepted checkpoint. Reconcile the
        // original saved question against its actual native user entry, even
        // after cancellation, a lost reply, or a new run for the same question.
        const received=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.customType==='learning-station-changes'&&e.sourceEntry.details?.courseId===focus.courseId).map(e=>e.sourceEntry).filter(e=>manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-changes-request'&&r.data?.changesEntryId===e.id&&r.data.runId===e.details.runId&&r.data.userEntryId===e.details.userEntryId&&r.data.nativeSessionId===manager.getSessionId())&&manager.getEntries().some(u=>u.type==='custom'&&u.customType==='learning-station-user'&&u.data?.userEntryId===e.details.userEntryId&&manager.getEntry(u.data.nativeId)?.message?.role==='user')).at(-1);
        const body=materialBaseline(focus.asset?.id??focus.documentId).last;
        const focusBaseCommitOid=body?.range?.complete&&body.path===focus.path?body.gitCommitOid??null:null;
        const changes=await loadChanges({baseCommitOid:received?.details.checkpointCommitOid??null,focusBaseCommitOid});
        if(changes?.complete!==true||changes.checkpoint?.courseId!==focus.courseId||changes.checkpoint.checkpointCommitOid!==focus.commitHash||changes.checkpoint.materialTreeOid!==focus.materialTreeOid||changes.baseCommitOid!==(received?.details.checkpointCommitOid??null)||!Array.isArray(changes.changes))throw new NativeSessionError();
        if(Buffer.byteLength(JSON.stringify(changes))>512*1024)throw new NativeSessionError('LIMIT_EXCEEDED');
        if(containsSecret(changes,apiKey))throw new NativeSessionError('AUTH');
        const notificationChanges=changes.changes.filter(change=>change.path!==focus.path).map(({patch,patchBytes,patchBaseCommitOid,...summary})=>summary);
        if((changes.focused?.textUnavailable||focus.binary||['image','pdf','exercise'].includes(focus.type))&&(typeof changes.focused?.text==='string'||focus.paragraphs?.some(p=>p.text?.length)))throw new NativeSessionError('CONTEXT_CONFLICT');
        currentSources=typeof changes.focused?.text==='string'?[changes.focused]:[];
        for(const source of currentSources)if(sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
        if(currentSources.length+changes.changes.length+(changes.checkpoint.excludedFiles?.length??0)+inventory.entries.filter(e=>e.kind!=='material').length>256)throw new NativeSessionError('LIMIT_EXCEEDED');
        inventory.entries=inventory.entries.filter(e=>e.kind!=='material');
        for(const source of currentSources)inventory.entries.push({id:source.id,kind:'material',sourceId:source.id,path:source.path,version:source.version,includedChars:[...source.text].length,range:source.range,bodyStartChar:source.bodyStartChar??0,coordinateSpace:source.bodyStartChar?'serialized-document':'body'});
        if(!currentSources.length&&!changes.focused?.textUnavailable)inventory.entries.push({id:`focus:${focus.asset?.id??focus.documentId}`,kind:'material',status:'added',sourceId:focus.asset?.id??focus.documentId,path:focus.path,version:focus.documentVersion,includedChars:0,reason:'focus_metadata_only'});
        if(changes.focused?.textUnavailable)inventory.entries.push({id:changes.focused.id,kind:'material',status:'retained',sourceId:changes.focused.id,version:focus.documentVersion??focus.commitHash,includedChars:0,reason:`text_unavailable:${changes.focused.textUnavailable}`,fixedVersion:{courseId:focus.courseId,checkpointCommitOid:focus.commitHash,path:changes.focused.path,blobOid:changes.focused.gitBlobOid}});
        for(const change of notificationChanges)inventory.entries.push({id:`change:${change.path}`,kind:'material',status:change.status==='A'?'added':'updated',version:focus.materialTreeOid,includedChars:JSON.stringify(change).length,path:change.path,fileStatus:change.status,added:change.added,deleted:change.deleted,binary:change.binary,fixedVersion:change.fixedVersion,reason:change.patchUnavailable?'git_patch_non_utf8':change.binary?'git_binary_status':'git_name_status_numstat'});
        for(const [index,excluded] of (changes.checkpoint.excludedFiles??[]).entries())inventory.entries.push({id:`excluded:${index}:${excluded.assetId}`,kind:'material',status:'retained',...(excluded.assetId?{sourceId:excluded.assetId}:{}),version:focus.commitHash,includedChars:0,reason:`excluded:${excluded.reason}`,fileName:excluded.fileName});
        if(changes.changes.length||changes.exclusionsChanged){
          const notification={baseCommitOid:changes.baseCommitOid,checkpointCommitOid:focus.commitHash,changes:notificationChanges};
          bridge.message('material_changes','非聚焦材料的文件变化摘要','learning-station-changes',notification,{courseId:focus.courseId,checkpointCommitOid:focus.commitHash,materialTreeOid:focus.materialTreeOid,runId:spec.runId,userEntryId:spec.userEntryId});
        }
      }
      for(const source of currentSources){
        if(source.projectionError)throw new NativeSessionError('CONTEXT_CONFLICT');
        if(typeof source.text!=='string'||sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
        const {active,last}=materialBaseline(source.id);
        let status=manager.getEntries().some(e=>e.type==='custom_message'&&e.customType==='learning-station-material'&&e.details?.sourceId===source.id)?'reloaded':'added',frames=[{kind:'full',version:source.version,textHash:source.textHash,text:source.text}];
        if(last&&source.range?.complete&&last?.range?.complete&&(!source.path||last.path===source.path)&&last.textHash===source.textHash){status='retained';frames=[];}
        else if(source.range?.complete&&last?.range?.complete&&(!source.path||last.path===source.path)&&active.length<8){
          if(source.gitChange&&last.gitCommitOid===source.gitChange.baseCommitOid&&last.gitBlobOid===source.gitChange.baseBlobOid){frames=[{kind:'diff',version:source.version,baseHash:last.textHash,textHash:source.textHash,unifiedDiff:source.gitChange.unifiedDiff}];status='updated';}
          else {const at=source.frames?.findIndex(f=>f.textHash===last.textHash)??-1;if(at>=0&&source.frames.length>at+1){frames=source.frames.slice(at+1);status='updated';}}
        }
        if(frames.length){
          if(frames[0].kind==='full')for(const entry of active)bridge.exclude(entry.sourceEntry.id);
          bridge.message('focus','已授权材料的完整正文或真实 Git 差量','learning-station-material',{type:'automatic_material_projection',id:source.id,...(source.path?{path:source.path}:{}),version:source.version,range:source.range,...(source.bodyStartChar?{bodyStartChar:source.bodyStartChar}:{}),frames},{sourceId:source.id,...(source.path?{path:source.path}:{}),textHash:source.textHash,version:source.version,range:source.range,...(source.gitCommitOid?{gitCommitOid:source.gitCommitOid,gitBlobOid:source.gitBlobOid}:{})});
        }
        const item=inventory.entries.find(e=>e.sourceId===source.id&&e.kind==='material');if(item){item.includedChars=[...source.text].length;item.range=source.range;item.bodyStartChar=source.bodyStartChar??0;item.coordinateSpace=source.bodyStartChar?'serialized-document':'body';item.status=status;item.reason=status==='retained'?'native_projection_contains_baseline':status==='updated'?'native_projection_verified_diff':'native_baseline_reload';}
      }
      for(const item of inventory.entries)if(item.kind==='turn'&&(item.id==='focus'||item.id.startsWith('focus:'))&&currentSources.length){item.includedChars=0;item.reason='focus_location';}
      if(envelope.focus) {
        const f=envelope.focus;
        const location={documentId:f.documentId,sourceDocumentId:f.sourceDocumentId,path:f.path,type:f.type,version:f.documentVersion,commitHash:f.commitHash,view:f.view};
        if(f.asset)location.asset={id:f.asset.id};
        if(f.exercise)location.exercise=f.exercise;
        if(f.paragraphs?.length)location.paragraphs=currentSources.length?f.paragraphs.map(({text,...range})=>range):f.paragraphs;
        bridge.message('focus','当前聚焦位置与可见范围','learning-station-focus',location);
      }
      return inventory;
}
