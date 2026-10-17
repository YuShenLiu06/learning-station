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
const set=(el,value)=>{Object.getOwnPropertyDescriptor(el.constructor.prototype,'value').set.call(el,value);el.dispatchEvent(new dom.window.Event(el.tagName==='SELECT'?'change':'input',{bubbles:true}));},submit=()=>document.querySelector('form').dispatchEvent(new dom.window.Event('submit',{bubbles:true,cancelable:true})),inlineError=()=>document.querySelector('.inline-error')?.textContent;
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
test('InsertContent shows nearby errors and inserts nothing for unsafe, empty or oversized input',async()=>{
 const inserted=[];
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.InsertContent,{onInsert:text=>inserted.push(text)})));
  const kind=()=>document.querySelector('select'),source=()=>document.querySelector('input[aria-label="嵌入来源"]');
  for(const url of ['http://example.com','https://user:pw@example.com','https://127.0.0.1','https://localhost:8443','ftp://example.com','https://example.com:6000']){
   await React.act(async()=>{set(kind(),'web');set(source(),url);submit();});
   assert.equal(inlineError(),'请使用无凭据的公共 HTTPS 原链接。',url);assert.equal(inserted.length,0,url);
  }
  await React.act(async()=>{set(kind(),'video');submit();});
  assert.equal(inlineError(),'请使用无凭据的公共 HTTPS 原链接。');assert.equal(inserted.length,0);
  await React.act(async()=>{set(kind(),'web');set(source(),'https://example.com/embed');submit();});
  assert.equal(inlineError(),undefined);assert.equal(inserted.length,1);assert.match(inserted[0],/learning-embed/);
  await React.act(async()=>set(kind(),'html'));
  const fileInput=document.querySelector('input[type="file"]');
  Object.defineProperty(fileInput,'files',{value:[new dom.window.File(['x'.repeat(ui.MAX_HTML_CHARS+1)],'big.html',{type:'text/html'})],configurable:true});
  await React.act(async()=>fileInput.dispatchEvent(new dom.window.Event('change',{bubbles:true})));
  assert.equal(inlineError(),'HTML 文件最多200KB。');
  await React.act(async()=>{set(document.querySelector('textarea'),'x'.repeat(ui.MAX_HTML_CHARS+1));submit();});
  assert.equal(inlineError(),'请填写 HTML，最多200000字符。');assert.equal(inserted.length,1);
 }finally{await React.act(async()=>root.unmount());}
});
test('html fences inside nested blockquotes stay plain text and never become embeds',async()=>{
 const quoted='> 外层\n> > ```html\n> > <p onclick="alert(1)">引用内演示</p>\n> > ```\n';
 assert.deepEqual(ui.contentSegments(quoted).map(s=>s.kind),['markdown']);
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:quoted})));
  assert.equal(document.querySelector('.embedded-content,iframe'),null);
  assert(document.body.textContent.includes('<p onclick="alert(1)">引用内演示</p>'));
 }finally{await React.act(async()=>root.unmount());}
});
test('AI html fence beyond MAX_HTML_CHARS falls back to a plain code block',async()=>{
 const over='<p>'+'x'.repeat(ui.MAX_HTML_CHARS)+'</p>';
 assert.deepEqual(ui.contentSegments('```html\n'+over+'\n```').map(s=>s.kind),['markdown']);
 assert.equal(ui.contentSegments('```html\n'+'x'.repeat(ui.MAX_HTML_CHARS)+'\n```').at(-1).kind,'embed');
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:'```html\n'+over+'\n```'})));
  assert.equal(document.querySelector('.embedded-content,iframe'),null);
  assert(document.querySelector('pre'));assert(document.body.textContent.includes('x'.repeat(100)));
 }finally{await React.act(async()=>root.unmount());}
});
test.after(async()=>{await unlink(file);dom.window.close();});
