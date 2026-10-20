import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {unlink} from 'node:fs/promises';
import {JSDOM} from 'jsdom';
const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,IS_REACT_ACT_ENVIRONMENT:true});
const file=new URL(`./.independent-markdown-${process.pid}.mjs`,import.meta.url);
await build({stdin:{contents:"export {Markdown} from './src/Markdown'; export {captureFocus} from './src/focus';",resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
const React=await import('react'),{createRoot}=await import('react-dom/client'),ui=await import(file.href);
const root=createRoot(document.getElementById('root'));
test('CRLF visible second paragraph must freeze its exact raw source range',async()=>{
 const body='first\r\n\r\nsecond\r\n';
 await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:body,focus:true})));
 const parent=document.getElementById('root');parent.getBoundingClientRect=()=>({top:0,bottom:100,height:100});
 const blocks=[...parent.querySelectorAll('[data-focus-start]')];
 blocks.forEach(el=>el.getBoundingClientRect=()=>el.textContent.includes('second')?({top:10,bottom:50,height:40}):({top:200,bottom:220,height:20}));
 const input=ui.captureFocus('synthetic',body,'v1','read',parent);
 const actual=input.ranges.map(r=>body.slice(r.start,r.end)).join('');
 assert.equal(actual,'second\r\n');
});
test('Focus decoration preserves document-wide Markdown reference resolution',async()=>{
 const body='[ref]: https://example.com\n\nVisible [link][ref].\n';
 await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:body})));
 const before=document.getElementById('root').textContent;
 await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:body,focus:true})));
 const after=document.getElementById('root').textContent;
 assert.equal(after,before);
});
test('LF/CRLF/CR, emoji, list and code ranges map to exact raw text and preserve rendered semantics',async()=>{
 for(const separator of ['\n','\r\n','\r']) for(const fragment of ['Emoji 😀 paragraph.','- One\n- Two','```txt\nraw 😀 code\n```','[ref]: https://example.com\n\nLink [label][ref] and ![image][ref].','[ref]: https://example.com\n\n![asset](asset:local_image) and [label][ref].']) {
  const body=('Intro\n\n'+fragment+'\n').replaceAll('\n',separator);
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:body})));const before=document.getElementById('root').textContent;
  await React.act(async()=>root.render(React.createElement(ui.Markdown,{text:body,focus:true})));assert.equal(document.getElementById('root').textContent,before);
  const parent=document.getElementById('root');parent.getBoundingClientRect=()=>({top:0,bottom:100,height:100});
  for(const block of parent.querySelectorAll('[data-focus-start]')){const raw=body.slice(Number(block.dataset.focusStart),Number(block.dataset.focusEnd));assert.equal(raw.replace(/\r\n|\r/gu,'\n').includes('Intro'),block.textContent.includes('Intro'));block.getBoundingClientRect=()=>block.textContent.includes('Intro')?({top:200,bottom:220,height:20}):({top:10,bottom:50,height:40});}
  const input=ui.captureFocus('synthetic',body,'v1','read',parent);for(const range of input.ranges)assert.equal(body.slice(range.start,range.end).replace(/\r\n|\r/gu,'\n').includes('Intro'),false);
 }
});
test.after(async()=>{await React.act(async()=>root.unmount());dom.window.close();await unlink(file).catch(()=>{});});
