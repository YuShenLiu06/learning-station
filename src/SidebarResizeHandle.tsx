import { useEffect, useRef, useState } from 'react';
import { clampSidebarWidth } from './sidebarWidth';
export function SidebarResizeHandle({width,onChange}:{width:number;onChange:(value:number)=>void}) {
  const ref=useRef<HTMLDivElement>(null);
  const active=useRef<{id:number;x:number;width:number;maximum:number;last:number}|null>(null);
  const callback=useRef(onChange);callback.current=onChange;
  const [dragging,setDragging]=useState(false);const [bounds,setBounds]=useState({rendered:width,maximum:480});
  function measure(){const sidebar=ref.current?.parentElement;const main=sidebar?.closest('.app-shell')?.querySelector('.main-area');const rendered=sidebar?.getBoundingClientRect().width||width;return {rendered:Math.round(rendered),maximum:Math.floor(Math.max(180,Math.min(480,rendered+(main?.clientWidth||350)-350)))};}
  function release(){const pointer=active.current;active.current=null;setDragging(false);if(pointer&&ref.current?.hasPointerCapture?.(pointer.id))ref.current.releasePointerCapture(pointer.id);}
  useEffect(()=>{
    const move=(event:PointerEvent)=>{const pointer=active.current;if(!pointer||pointer.id!==event.pointerId)return;if(!(event.buttons&1)){release();return;}if(event.cancelable)event.preventDefault();const value=clampSidebarWidth(pointer.width+event.clientX-pointer.x,pointer.maximum);if(value!==pointer.last){pointer.last=value;callback.current(value);}};
    const end=(event:PointerEvent)=>{if(active.current?.id===event.pointerId)release();};
    window.addEventListener('pointermove',move,{capture:true,passive:false});window.addEventListener('pointerup',end,true);window.addEventListener('pointercancel',end,true);window.addEventListener('blur',release);
    return()=>{window.removeEventListener('pointermove',move,true);window.removeEventListener('pointerup',end,true);window.removeEventListener('pointercancel',end,true);window.removeEventListener('blur',release);active.current=null;};
  },[]);
  useEffect(()=>{const update=()=>{const value=measure();setBounds(old=>old.rendered===value.rendered&&old.maximum===value.maximum?old:value);};update();window.addEventListener('resize',update);const observer=typeof ResizeObserver==='undefined'?null:new ResizeObserver(update);const sidebar=ref.current?.parentElement;const main=sidebar?.closest('.app-shell')?.querySelector('.main-area');if(sidebar)observer?.observe(sidebar);if(main)observer?.observe(main);return()=>{observer?.disconnect();window.removeEventListener('resize',update);};},[width]);
  return <div ref={ref} className={`sidebar-resize-handle ${dragging?'resizing':''}`} role="separator" aria-label="调整目录宽度" aria-orientation="vertical" aria-valuemin={180} aria-valuemax={bounds.maximum} aria-valuenow={bounds.rendered} tabIndex={0} title="拖动目录边缘调整宽度；左右方向键微调" onPointerDown={event=>{if(event.button!==0)return;event.preventDefault();const value=measure();active.current={id:event.pointerId,x:event.clientX,width:value.rendered,maximum:value.maximum,last:value.rendered};setDragging(true);try{event.currentTarget.setPointerCapture(event.pointerId);}catch{/* Window capture listeners preserve the drag when native capture is unavailable. */}}} onLostPointerCapture={release} onKeyDown={event=>{if(!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;event.preventDefault();const value=measure();const next=clampSidebarWidth(event.key==='Home'?180:event.key==='End'?value.maximum:value.rendered+(event.key==='ArrowRight'?1:-1)*(event.shiftKey?40:16),value.maximum);if(next!==value.rendered)callback.current(next);}}/>;
}
