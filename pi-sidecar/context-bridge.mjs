// Single application-supplement boundary. Pi still owns messages, tools and compaction.
const order = ['current_time','references','focus','material_changes','exploration'];
export function decodeBridgeInput(input) {
  if (!Array.isArray(input.bridge)) return input; // Existing private request fixtures/older callers.
  const values={};const seen=new Map();
  for(const part of input.bridge) {
    if(!part||typeof part.key!=='string'||typeof part.description!=='string'||!part.description.trim())throw Object.assign(new Error('INVALID_CONTEXT_REQUEST'),{code:'INVALID_INPUT'});
    const fingerprint=JSON.stringify([part.description,part.value]);
    if(seen.has(part.key)&&seen.get(part.key)!==fingerprint)throw Object.assign(new Error('CONTEXT_CONFLICT'),{code:'CONTEXT_CONFLICT'});
    seen.set(part.key,fingerprint);values[part.key]=part.value;
  }
  return { ...input, question: values.question, focus: values.focus, nativeMaterials: values.materials ?? [], nativeReferences: values.references ?? '', currentTime: values.current_time, exploration: values.exploration };
}
export class ContextBridge {
  constructor(manager, identity) { this.manager=manager; this.identity=identity; this.pending=[]; this.removed=new Set(); }
  exclude(id) { this.removed.add(id); }
  message(key,description,customType,value,details={}) {
    if (!order.includes(key) || !description) throw new Error('INVALID_CONTEXT_REQUEST');
    this.pending.push({key,description,customType,value,details});
  }
  commit() {
    if(Buffer.byteLength(JSON.stringify(this.pending.map(p=>p.value)))>2*1024*1024)throw Object.assign(new Error('LIMIT_EXCEEDED'),{code:'LIMIT_EXCEEDED'});
    for (const id of this.removed) this.manager.appendContextEdit(id,null);
    for (const part of this.pending.sort((a,b)=>order.indexOf(a.key)-order.indexOf(b.key))) {
      this.manager.appendCustomMessageEntry(part.customType,typeof part.value==='string'?part.value:JSON.stringify(part.value),false,{...part.details,...this.identity,bridgeKey:part.key,bridgeDescription:part.description});
    }
    this.pending=[];this.removed.clear();
  }
  // Operate only on registered application messages supplied by Pi's own projection.
  // Keep every native user/tool/assistant entry and its relative order unchanged.
  ordered(messages) {
    const supplements=messages.filter(m=>m.role==='custom'&&m.details?.bridgeKey);
    if (!supplements.length) return messages;
    supplements.sort((a,b)=>order.indexOf(a.details.bridgeKey)-order.indexOf(b.details.bridgeKey));
    const ordinary=messages.filter(m=>!supplements.includes(m));
    let at=ordinary.findLastIndex(m=>m.role==='user');
    if(at<0)at=ordinary.length;
    return [...ordinary.slice(0,at),...supplements,...ordinary.slice(at)];
  }
  registrations() {
    return this.manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.details?.bridgeKey).sort((a,b)=>order.indexOf(a.sourceEntry.details.bridgeKey)-order.indexOf(b.sourceEntry.details.bridgeKey)).map((entry,position)=>({key:entry.sourceEntry.details.bridgeKey,description:entry.sourceEntry.details.bridgeDescription,value:entry.sourceEntry.details.bridgeKey==='references'?entry.sourceEntry.content:JSON.parse(entry.sourceEntry.content),position,runId:entry.sourceEntry.details.runId,userEntryId:entry.sourceEntry.details.userEntryId}));
  }
}

// Single-turn tasks use the same registered projection without creating a session or history.
export function projectStandalone(parts) {
  const registrations=parts.sort((a,b)=>order.indexOf(a.key)-order.indexOf(b.key));
  const sources=registrations.map(part=>({value:typeof part.value==='string'?part.value:JSON.stringify(part.value),origin:'bridge',name:'learning-station-turn',registrationKey:part.key,description:part.description,sourcePath:`bridge/${part.key}`}));
  return {registrations,sources,messages:sources.map(source=>({role:'user',content:source.value,timestamp:Date.now()}))};
}
