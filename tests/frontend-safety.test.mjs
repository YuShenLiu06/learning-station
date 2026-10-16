import { sessionMessages } from '../src/sessionMessages.ts';
import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { composerKeyAction } from '../src/keyboard.ts';
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
test('Enter sends once while Shift+Enter and Alt+Enter keep native multiline input', () => {
  assert.equal(composerKeyAction({key:'Enter'}), 'send');
  assert.equal(composerKeyAction({key:'Enter',repeat:true}), 'block');
  assert.equal(composerKeyAction({key:'Enter',shiftKey:true}), 'native');
  assert.equal(composerKeyAction({key:'Enter',shiftKey:true,repeat:true}), 'native');
  assert.equal(composerKeyAction({key:'Enter',altKey:true}), 'native');
  assert.equal(composerKeyAction({key:'a'}), 'native');
});
test('Chinese IME Enter commits text without sending, including WebKit commit ordering', () => {
  assert.equal(composerKeyAction({key:'Enter',isComposing:true}), 'native');
  assert.equal(composerKeyAction({key:'Enter',keyCode:229}), 'native');
  assert.equal(composerKeyAction({key:'Enter',isComposing:false,keyCode:13},true), 'native');
  assert.equal(composerKeyAction({key:'Enter',isComposing:false,keyCode:13},false), 'send');
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

 test('local usage policy errors are distinct from connection failures, including plain CoreError objects', async () => {
  const {modelAccessMessage,modelFailureMessage}=await import('../src/modelPolicy.ts');
  const settings={configured:true,provider:'custom',baseUrl:'https://example.invalid/coding',model:'test',credentialStored:true,usageGate:'PLAN_USAGE_UNCONFIRMED'};
  assert.match(modelAccessMessage(settings),/专用通道.*未发送网络请求/);
  assert.equal(modelAccessMessage({...settings,usageGate:''}),'');
  const policy=modelFailureMessage({code:'PLAN_USAGE_UNCONFIRMED',message:'old gate wording'},'test');
  assert.match(policy,/本地用途限制/);
  assert.doesNotMatch(policy,/连接测试未通过|测试未完成|网络失败/);
  assert.match(modelFailureMessage({code:'NETWORK',message:'connection refused'},'test'),/^网络请求未完成（学习站诊断 · NETWORK）：connection refused$/);
  assert.match(modelFailureMessage({code:'MODEL_RESPONSE',message:'safe application diagnostic'},'test'),/^响应解析未完成（学习站诊断 · MODEL_RESPONSE）：safe application diagnostic$/);
  assert.match(modelFailureMessage(Object.assign(new Error('gate'),{code:'PLAN_USAGE_UNCONFIRMED'}),'generation'),/未发送网络请求/);
});


test('request targets preserve explicit OpenAI prefixes and exact Anthropic SDK path without modifying settings', async () => {
  const {modelRequestTarget}=await import('../src/modelPolicy.ts');
  const input={provider:'custom',protocol:'openai-completions',baseUrl:'https://example.test:37174',model:'unchanged-model'};
  const snapshot={...input};
  assert.equal(modelRequestTarget(input),'https://example.test:37174/chat/completions');
  assert.equal(modelRequestTarget({...input,baseUrl:'https://example.test:37174/v1/'}),'https://example.test:37174/v1/chat/completions');
  assert.equal(modelRequestTarget({...input,protocol:'anthropic-messages',baseUrl:'https://example.test/proxy/'}),'https://example.test/proxy/v1/messages?beta=true');
  assert.equal(modelRequestTarget({...input,protocol:'anthropic-messages',baseUrl:'http://127.0.0.1:56789'}),'http://127.0.0.1:56789/v1/messages?beta=true');
  assert.deepEqual(input,snapshot);
});
test('Anthropic endpoint validation prevents duplicate paths and keeps credential binding protocol-specific', async () => {
  const {modelFormError,modelRequestTarget,modelBindingChanged}=await import('../src/modelPolicy.ts');
  const old={configured:true,provider:'custom',protocol:'openai-completions',baseUrl:'https://example.test',model:'same-model',credentialStored:true,usageGate:''};
  const changed={provider:'custom',protocol:'anthropic-messages',baseUrl:old.baseUrl,model:old.model,apiKey:''};
  assert.equal(modelBindingChanged(old,changed),true);
  assert.match(modelFormError(changed,old),/新密钥/);
  assert.equal(modelFormError({...changed,apiKey:'public-fixture-key'},old),'');
  assert.equal(modelFormError({...changed,apiKey:''},{...old,protocol:'anthropic-messages'}),'');
  assert.match(modelFormError({...changed,provider:'glm-pi',apiKey:'public-fixture-key'},old),/仅支持.*OpenAI/);
  for (const suffix of ['/v1','/v1/','/messages','/v1/messages']) {
    const input={...changed,baseUrl:old.baseUrl+suffix,apiKey:'public-fixture-key'};
    assert.match(modelFormError(input,old),/不要以 \/v1 或 \/messages 结尾/);
    assert.equal(modelRequestTarget(input),'');
  }
  for (const baseUrl of ['https://example.test?key=not-real','https://example.test#fragment','https://user:pass@example.test','http://example.test']) {
    assert.notEqual(modelFormError({...changed,baseUrl,apiKey:'public-fixture-key'},old),'');
    assert.equal(modelRequestTarget({...changed,baseUrl}),'');
  }
});
