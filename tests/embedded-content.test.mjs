import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {JSDOM} from 'jsdom';
import {unlink} from 'node:fs/promises';
const dom=new JSDOM('<div id="root"></div>',{url:'https://example.com',pretendToBeVisual:true});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,IS_REACT_ACT_ENVIRONMENT:true});
Object.defineProperty(globalThis,'navigator',{value:dom.window.navigator,configurable:true});
const file=new URL(`./.embed-${process.pid}.mjs`,import.meta.url);
await build({stdin:{contents:"export * from './src/embedPolicy';export * from './src/EmbeddedContent';export {Markdown} from './src/Markdown';",resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
const ui=await import(file.href),React=await import('react'),{createRoot}=await import('react-dom/client');
test('original HTML with arbitrary fence and remote URL survive Markdown round trip',()=>{
 for(const value of [{kind:'html',source:'原文件.html',content:'<p>```\n~~~\n中文</p><script>alert(1)</script>'},{kind:'web',source:'https://example.com/?a=1&b=2'},{kind:'video',source:'https://www.youtube.com/embed/test'}]){
   const segments=ui.contentSegments(ui.embedMarkdown(value));assert.deepEqual(segments.find(s=>s.kind==='embed').value,value);
 }
 for(const url of ['file:///etc/passwd','javascript:alert(1)','http://example.com','https://127.0.0.1','https://[::1]','https://host.local','https://a:b@example.com','https://example.com:6000'])assert.equal(ui.safeRemoteUrl(url),undefined,url);
});
test('HTML preview strips active DOM and encloses original text in a zero-capability CSP sandbox',async()=>{
 const malicious='<script>parent.pwned=1</script><img src="file:///tmp/private"><iframe src="https://example.com"></iframe><form action="ipc:"><input></form><meta http-equiv="refresh" content="0;url=https://example.com"><a href="javascript:alert(1)">x</a>';
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:'```html\n'+malicious+'\n```'})));
  assert.equal(document.querySelector('iframe'),null);assert(document.body.textContent.includes(malicious));
  await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='打开预览').click());
  const frame=document.querySelector('iframe');assert.equal(frame.getAttribute('sandbox'),'');
  assert.match(frame.srcdoc,/script-src 'none'/);assert.match(frame.srcdoc,/connect-src 'none'/);
  const isolated=new JSDOM(frame.srcdoc).window.document;
  assert.equal(isolated.querySelector('script,iframe,form,a[href],meta[http-equiv="refresh"]'),null);
  assert.equal(document.querySelector('script'),null);
  await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='重新加载').click());
  assert.notEqual(frame,document.querySelector('iframe'));
 }finally{await React.act(async()=>root.unmount());}
});
test('remote content is lazy, opaque, without native permissions and always has fallback',async()=>{
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:ui.embedMarkdown({kind:'video',source:'https://example.com/embed/video'})})));
  assert.equal(document.querySelector('iframe'),null);
  await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='打开预览').click());
  const frame=document.querySelector('iframe');assert.equal(frame.getAttribute('sandbox'),'allow-scripts');assert.equal(frame.getAttribute('referrerpolicy'),'no-referrer');
  assert(!frame.getAttribute('sandbox').includes('same-origin'));assert(document.body.textContent.includes('在浏览器打开原链接'));assert(document.body.textContent.includes('站点禁止嵌入'));
 }finally{await React.act(async()=>root.unmount());}
});
test.after(async()=>{await unlink(file);dom.window.close();});
