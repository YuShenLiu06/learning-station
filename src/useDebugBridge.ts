import { useEffect, useRef, useState } from 'react';
import { call } from './api';
export function useDebugBridge(sequence?: number) {
  const current=useRef(sequence); current.current=sequence;
  const [value,setValue]=useState<{complete:boolean}|null>(null);
  const [valueSequence,setValueSequence]=useState<number>();
  const [error,setError]=useState(''); const [saved,setSaved]=useState(''); const [busy,setBusy]=useState(false);
  useEffect(()=>{let active=true;setValue(null);setError('');setSaved('');
    if(sequence!==undefined)void call<{complete:boolean}>('request_debug_decode',{sequence}).then(value=>{if(active){setValueSequence(sequence);setValue(value);}},error=>{if(active)setError(error instanceof Error?error.message:'解码读取失败，请重新采集后重试。');});
    return()=>{active=false;};
  },[sequence]);
  async function save() {
    if(busy||sequence===undefined)return;const target=sequence;setBusy(true);setError('');setSaved('');
    try {const result=await call<{path:string;complete:boolean}>('request_debug_export',{sequence:target});
      if(current.current===target)setSaved('已导出'+(result.complete?'':'（不完整，见文件说明）')+'：'+result.path);
    }catch(error){if(current.current===target)setError(error instanceof Error?error.message:'导出失败，请重试。');}finally{setBusy(false);}
  }
  return {value:valueSequence===sequence?value:null,error,saved,busy,save};
}
