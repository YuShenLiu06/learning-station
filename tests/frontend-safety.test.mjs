import { sessionMessages } from '../src/sessionMessages.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { isSubmitKey } from '../src/keyboard.ts';
import { draftInputsDiffer, questionForSection, needsDraftChoice, unbackedSectionHasInput } from '../src/draftPolicy.ts';

const dom = new JSDOM('');
globalThis.window = dom.window;
const { sanitizeMarkdown } = await import('../src/sanitize.ts');
function render(source) { const element = dom.window.document.createElement('div'); element.innerHTML = sanitizeMarkdown(source); return element; }

test('untrusted Markdown keeps prose while removing scripts, event handlers, SVG and native command links', () => {
  const result = render('# 课程\n\n正文 **still readable**\n<script>alert(1)</script><img src=x onerror=alert(2)><iframe src="https://example.com"></iframe><svg onload=alert(3)></svg><a href="tauri://localhost" onclick=alert(4)>native</a>\n\n[run](javascript:alert(5))');
  assert.match(result.textContent, /课程/);
  assert.match(result.textContent, /still readable/);
  assert.equal(result.querySelectorAll('script,img,iframe,svg,a,[onerror],[onclick],[onload],[href]').length, 0);
  assert.equal(result.querySelector('strong').textContent, 'still readable');
});
test('nested malformed markup cannot expose forms or executable attributes', () => {
  const result = render('<math><mtext><table><mglyph><style><!--</style><img title="--><img src=x onerror=alert(1)>"><form action="tauri://localhost"><input autofocus onfocus=alert(2)></form>');
  for (const element of result.querySelectorAll('*')) for (const name of element.getAttributeNames()) assert.ok(!/^on|src|href|action|autofocus$/i.test(name));
  assert.equal(result.querySelectorAll('form,input,style,math').length, 0);
});
test('source URLs and external images cannot make automatic network requests', () => {
  const result = render('![remote](https://example.com/private-image)\n\n[link](https://example.com)\n\n<video poster="https://example.com/a"><source src="https://example.com/b"></video>');
  assert.equal(result.querySelectorAll('[src],[href],[poster],video,source,img').length, 0);
  assert.match(result.textContent, /link/);
});
test('plain Enter and Chinese IME composition never submit a question', () => {
  assert.equal(isSubmitKey({key:'Enter',metaKey:false,ctrlKey:false}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,isComposing:true}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,keyCode:229}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,isComposing:false,keyCode:13}), true);
  assert.equal(isSubmitKey({key:'Enter',metaKey:false,ctrlKey:true}), true);
});
test('late draft reads require a choice before replacing changed local answer or reason', () => {
  assert.equal(needsDraftChoice({answer:'new',reason:''},{answer:'old',reason:''},true), true);
  assert.equal(needsDraftChoice({answer:'same',reason:'new cause'},{answer:'same',reason:'old cause'},true), true);
  assert.equal(needsDraftChoice({answer:'same',reason:''},{answer:'same',reason:''},true), false);
  assert.equal(needsDraftChoice({answer:'',reason:''},{answer:'saved',reason:'saved'},false), false);
});
test('a question alone prevents closing after its draft read failed', () => {
  assert.equal(unbackedSectionHasInput(false,'saved','saved','unsent question'),true);
  assert.equal(unbackedSectionHasInput(false,'edited','saved',''),true);
  assert.equal(unbackedSectionHasInput(false,'saved','saved',''),false);
  assert.equal(unbackedSectionHasInput(true,'saved','saved','unsent question'),false);
});

test('section recovery compares unsent questions even when the body is unchanged', () => {
  assert.equal(draftInputsDiffer('section:a', {body:'same',question:'new question'}, {body:'same',question:'old question'}),true);
  assert.equal(draftInputsDiffer('section:a', {body:'same',question:''}, {body:'same',question:'old question'}),true);
  assert.equal(draftInputsDiffer('section:a', {body:'same',question:'same'}, {body:'same',question:'same'}),false);
});
test('workspace recovery compares its own title/body independently of section fields', () => {
  assert.equal(draftInputsDiffer('workspace', {importTitle:'new',importBody:'same',question:'ignored'}, {importTitle:'old',importBody:'same',question:'different'}),true);
  assert.equal(draftInputsDiffer('workspace', {importTitle:'same',importBody:'new'}, {importTitle:'same',importBody:'old'}),true);
  assert.equal(draftInputsDiffer('workspace', {importTitle:'same',importBody:'same',body:'a'}, {importTitle:'same',importBody:'same',body:'b'}),false);
});

test('importing or switching sections restores only that section question, never the previous composer', () => {
  const drafts = {'old-section':'old unsent question','existing-section':'its own question'};
  assert.equal(questionForSection(drafts,'new-imported-section'),'');
  assert.equal(questionForSection(drafts,'existing-section'),'its own question');
  assert.equal(questionForSection(drafts,'old-section'),'old unsent question');
  assert.deepEqual(drafts,{'old-section':'old unsent question','existing-section':'its own question'});
});

test('Pi tool notes are persisted learning records awaiting confirmation, never user quotations or inferred saves', () => {
  const entries = [
    {id:'u',kind:'user',content:'my wording',createdAt:'now'},
    {id:'a',kind:'assistant',content:'已为你保存',createdAt:'now'},
    {id:'q',kind:'note',content:'AI整理的待解问题',createdAt:'now',data:{origin:'pi-tool',question:true,needsConfirmation:true}},
    {id:'c',kind:'note',content:'card',createdAt:'now',data:{reviewCard:true}},
    {id:'s',kind:'note',content:'AI cause suggestion',createdAt:'now',data:{origin:'pi-tool',errorSuggestion:true,needsConfirmation:true}},
  ];
  const mapped = sessionMessages(entries, [{title:'source'}]);
  assert.deepEqual(mapped.map(x=>[x.id,x.role,x.needsConfirmation]),[['u','user',false],['a','assistant',false],['q','note',true],['s','note',true]]);
  assert.equal(mapped[2].content,'AI整理的待解问题');
  assert.equal(mapped[2].sources,undefined);
  assert.equal(sessionMessages([entries[1]],[]).length,1);
  assert.equal(sessionMessages([entries[1]],[])[0].role,'assistant');
});

test('custom model configuration is editable while Coding Plan gate remains provider-specific', async () => {
  const {modelAccessReason,modelFormError,modelBindingChanged}=await import('../src/modelPolicy.ts');
  const base={configured:false,provider:'custom',protocol:'openai-completions',baseUrl:'',model:'',credentialStored:false,usageGate:''};
  assert.equal(modelAccessReason(base),'');
  assert.equal(modelAccessReason({...base,provider:'glm-pi',usageGate:undefined}),'PLAN_USAGE_UNCONFIRMED');
  const input={provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1:53990/v1',model:'qa-custom-model',apiKey:'local-fixture-not-a-real-key'};
  assert.equal(modelFormError(input,base),'');
  assert.match(modelFormError({...input,baseUrl:'not-a-url'},base),/地址/);
  assert.match(modelFormError({...input,protocol:'unsupported'},base),/仅支持/);
  assert.match(modelFormError({...input,baseUrl:'http://127.0.0.1/v1/chat/completions'},base),/基础地址/);
  const saved={...base,configured:true,credentialStored:true,baseUrl:input.baseUrl,model:input.model};
  assert.equal(modelFormError({...input,apiKey:''},saved),'');
  assert.equal(modelBindingChanged(saved,{...input,baseUrl:'http://127.0.0.1:9999/v1'}),true);
  assert.match(modelFormError({...input,baseUrl:'http://127.0.0.1:9999/v1',apiKey:''},saved),/新密钥/);
});
test('independent exploration rejects a linked session or source manifest', async () => {
  const {assertExplorationScope,validFontSize}=await import('../src/explorationPolicy.ts');
  assert.doesNotThrow(()=>assertExplorationScope({session:{sectionId:''},context:{manifest:[]}},false));
  assert.throws(()=>assertExplorationScope({session:{sectionId:'course'},context:{manifest:[]}},false));
  assert.throws(()=>assertExplorationScope({session:{sectionId:''},context:{manifest:[{id:'course'}]}},false));
  assert.throws(()=>assertExplorationScope({session:{sectionId:'different'},context:{manifest:[]}},true,'current'));
  assert.doesNotThrow(()=>assertExplorationScope({session:{sectionId:'current'},context:{manifest:[]}},true,'current'));
  assert.equal(validFontSize(12),true);assert.equal(validFontSize(24),true);assert.equal(validFontSize(25),false);assert.equal(validFontSize(16.5),false);
});
test('historical assistant prose cannot borrow the current session source manifest', () => {
  const messages=sessionMessages([{id:'old',kind:'assistant',content:'old answer',createdAt:'then'}],[{title:'unrelated latest source'}]);
  assert.equal(messages[0].sources,undefined);
});
