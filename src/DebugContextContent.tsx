import {useState} from 'react';
import {ChevronRight} from 'lucide-react';
import {DebugJsonView} from './DebugJsonView';
import {sourceContent,sourcePresentation,sourceReadingContent,sourceValue} from './requestDebug';
import type {DebugRecord,DebugSource} from './requestDebug';

// Presentation reads only this captured wire fragment; it never fetches current UI state.
export function DebugContextContent({record,source,reading}:{record:DebugRecord;source:DebugSource;reading?:boolean}){
 const [rawOpen,setRawOpen]=useState(false),presentation=sourcePresentation(record,source);
 if(!presentation)return <pre>{reading?sourceReadingContent(record,source):sourceContent(record,source)}</pre>;
 const raw=sourceValue(record,source);let value:unknown=raw;try{if(typeof raw==='string')value=JSON.parse(raw);}catch{}
 return <div className="debug-readable-context">
  {!!presentation.facts.length&&<dl className="debug-content-facts">{presentation.facts.map(([label,value])=><div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>}
  {presentation.notice&&<p className="debug-notice">{presentation.notice}</p>}
  {presentation.sections.map((section,index)=><section className="debug-content-section" key={index}><h5>{section.title}</h5><DebugJsonView format={section.format} text={section.text} label={section.title+' · '+source.path.join('/')}/></section>)}
  <button className="quiet-button debug-raw-toggle" aria-expanded={rawOpen} onClick={()=>setRawOpen(!rawOpen)}>{rawOpen?'收起完整实发数据':'查看完整实发数据'}<ChevronRight className="debug-chevron" size={13}/></button>
  {rawOpen&&<DebugJsonView value={value} label={'完整实发数据 · '+source.path.join('/')}/>}
 </div>;
}
