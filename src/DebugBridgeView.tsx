import { useEffect, useState } from 'react';
import { call } from './api';
import { DebugJsonView } from './DebugJsonView';
export function DebugBridgeView({sequence}: {sequence: number}) {
  const [value,setValue] = useState<{complete:boolean} | null>(null);
  const [error,setError] = useState('');
  const [saved,setSaved] = useState('');
  const [busy,setBusy] = useState(false);
  useEffect(()=>{let active=true;setValue(null);setError('');setSaved('');
    void call<{complete:boolean}>('request_debug_decode',{sequence}).then(value=>{if(active)setValue(value);},error=>{if(active)setError(error instanceof Error?error.message:'解码读取失败，请重新采集后重试。');});
    return()=>{active=false;};
  },[sequence]);
  async function save() {if(busy)return;setBusy(true);setError('');setSaved('');try{
    const result=await call<{path:string;complete:boolean}>('request_debug_export',{sequence});
    setSaved(`已导出${result.complete?'':'（不完整，见 JSON 中的完整性说明）'}：${result.path}`);
  }catch(error){setError(error instanceof Error?error.message:'导出失败，请重试。');}finally{setBusy(false);}}
  return <div className="debug-bridge-view">
    <p className="muted small">仅 ContextBridge。registrationSnapshot 是内部冻结输入；finalProjection 是最终实发片段，按消息路径及片段位置排序。后续拼装明确标为未注册；缺失快照不补造，未绑定来源不冒充桥内容。</p>
    {value&&<><div className="debug-export-row"><button className="primary-button" disabled={busy} onClick={()=>{void save();}}>{busy?'正在导出…':'导出ContextBridge解码JSON'}</button><span className={value.complete?'muted small':'inline-error small'}>{value.complete?'采集内容完整':'整体不完整：注册快照缺失 / 省略，或有未归属片段；详情见 JSON'}</span></div><p className="muted small">导出全部已捕获桥内容，不受折叠、搜索或屏幕范围影响。已脱敏；仍可能含私人正文。文件保存在本机 state/exports，关闭调试不会删除已导出文件。</p><DebugJsonView value={value} label="ContextBridge解码JSON"/></>}
    {!value&&!error&&<p role="status">正在解码已捕获的桥内容…</p>}
    {saved&&<p role="status" className="debug-export-result">{saved}</p>}
    {error&&<p role="alert" className="inline-error">{error}</p>}
  </div>;
}
