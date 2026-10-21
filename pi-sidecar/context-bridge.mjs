// Single application-supplement boundary. Pi still owns messages, tools and compaction.
const order = ['current_time','references','focus','material_changes','exploration'];
export const renderBridgePart=(description,value)=>typeof value==='string'?`${description}\n${value}`:JSON.stringify({description,...value});
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
      this.manager.appendCustomMessageEntry(part.customType,renderBridgePart(part.description,part.value),false,{...part.details,...this.identity,bridgeKey:part.key,bridgeDescription:part.description});
    }
    this.pending=[];this.removed.clear();
  }
  // Anchor each turn to Pi's durable user entry, never to matching question text
  // or to the latest user. Compaction remains authoritative when an anchor is gone.
  ordered(messages) {
    const anchors=new Map();
    for(const entry of this.manager.getEntries()) {
      if(entry.type!=='custom'||entry.customType!=='learning-station-user')continue;
      const native=this.manager.getEntry(entry.data?.nativeId);
      if(native?.type!=='message'||native.message.role!=='user')continue;
      const at=messages.indexOf(native.message);
      if(at>=0)anchors.set(entry.data.userEntryId,at);
    }
    const blocks=new Map(),moved=new Set();
    for(const message of messages) {
      if(message.role!=='custom'||!message.details?.bridgeKey)continue;
      const at=anchors.get(message.details.userEntryId);
      if(at===undefined)continue;
      if(!blocks.has(at))blocks.set(at,[]);
      blocks.get(at).push(message);moved.add(message);
    }
    return messages.flatMap((message,index)=>[
      ...(blocks.get(index)??[]).sort((a,b)=>order.indexOf(a.details.bridgeKey)-order.indexOf(b.details.bridgeKey)),
      ...(moved.has(message)?[]:[message]),
    ]);
  }
  registrations() {
    const projection=this.manager.buildSessionProjection(),ordered=this.ordered(projection.messages);
    return projection.entries.filter(e=>e.messages.length&&e.sourceEntry.details?.bridgeKey).sort((a,b)=>ordered.indexOf(a.messages[0])-ordered.indexOf(b.messages[0])).map((entry,position)=>({key:entry.sourceEntry.details.bridgeKey,description:entry.sourceEntry.details.bridgeDescription,value:entry.sourceEntry.content,position,runId:entry.sourceEntry.details.runId,userEntryId:entry.sourceEntry.details.userEntryId}));
  }
}

// Single-turn tasks use the same registered projection without creating a session or history.
export function projectStandalone(parts) {
  const registrations=parts.sort((a,b)=>order.indexOf(a.key)-order.indexOf(b.key));
  const sources=registrations.map(part=>({value:renderBridgePart(part.description,part.value),origin:'bridge',name:'learning-station-turn',registrationKey:part.key,description:part.description,sourcePath:`bridge/${part.key}`}));
  return {registrations,sources,messages:sources.map(source=>({role:'user',content:source.value,timestamp:Date.now()}))};
}
