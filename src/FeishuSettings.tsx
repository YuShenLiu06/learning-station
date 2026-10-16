import { useEffect, useState } from 'react';
import { Check, LoaderCircle, Send } from 'lucide-react';
import type { FeishuBotSettings, FeishuBotSnapshot } from './FeishuBotConfiguration';
import { FeishuBotConfiguration } from './FeishuBotConfiguration';
import { call } from './api';
import { feishuRequestId, feishuTestResult } from './feishuPolicy';
import type { UiDraft } from './types';
import type { FeishuTestResult as TestResult } from './feishuPolicy';
const attempt: { id: string; result: TestResult | null; pending: boolean } = { id: '', result: null, pending: false };
const attemptListeners = new Set<() => void>();
const publishAttempt = () => attemptListeners.forEach(listener => listener());
export function FeishuSettings() {
  const [bot,setBot]=useState<{settings:FeishuBotSettings|null;snapshot:FeishuBotSnapshot|null}>({settings:null,snapshot:null});
  const canSend=!!bot.settings?.configured&&bot.settings.deliveryEnabled&&bot.settings.runtimeAvailable&&!!bot.snapshot&&!bot.snapshot.processExited&&!bot.snapshot.outputClosed&&['receive_disabled','connecting','connected','disconnected','reconnecting'].includes(bot.snapshot.state);
  const [connection, setConnection] = useState<{ ready: boolean; reason: string } | null>(null);
  const [result, setResult] = useState<TestResult | null>(attempt.result);
  const [checking, setChecking] = useState(false);
  const [testPending, setTestPending] = useState(attempt.pending);
  const [draftReady,setDraftReady]=useState(false);
  const [requestError,setRequestError]=useState('');
  const busy = !draftReady ? 'draft' : testPending ? 'test' : checking ? 'status' : '';
  const [error, setError] = useState('');
  async function check() { setChecking(true); setError(''); try { setConnection(await call('feishu_status')); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } finally { setChecking(false); } }
  useEffect(() => { const sync = () => { setTestPending(attempt.pending); setResult(attempt.result); }; attemptListeners.add(sync); sync(); return () => { attemptListeners.delete(sync); }; }, []);
  async function restoreRequest(){setDraftReady(false);setRequestError('');try{const saved=await call<UiDraft|null>('draft_get',{key:'feishu-test'});if(saved?.data.requestId&&!attempt.pending){attempt.id=saved.data.requestId;attempt.result={status:'unknown',messageId:null,reason:'已找回上次请求编号；请显式核对结果，不新建发送编号。'};publishAttempt();}setDraftReady(true);}catch(error){setRequestError(`上次测试请求未读取，暂停发送：${error instanceof Error?error.message:String(error)}`);}}
  useEffect(() => { void check(); void restoreRequest(); }, []);
  async function test() {
    if (busy || attempt.pending || (!canSend&&attempt.result?.status!=='unknown')) return;
    attempt.pending = true;
    attempt.id = feishuRequestId(attempt.id, attempt.result, crypto.randomUUID());
    publishAttempt(); setError(''); setRequestError('');
    try{await call('draft_save',{key:'feishu-test',data:{requestId:attempt.id}});}catch(error){attempt.pending=false;publishAttempt();setDraftReady(false);setRequestError(`测试编号未备份，未发送消息：${error instanceof Error?error.message:String(error)}`);return;}
    try { const value = feishuTestResult(await call<TestResult>('feishu_test', { requestId: attempt.id })); attempt.result = value; setResult(value); }
    catch { const value: TestResult = { status: 'unknown', messageId: null, reason: '发送结果尚未确认；请先核对这次结果，避免重复发送。' }; attempt.result = value; setResult(value); }
    finally { attempt.pending = false; publishAttempt(); }
  }
  const success = result?.status === 'sent' && !!result.messageId;
  return <section className="feishu-settings" aria-label="飞书通知设置"><h2>飞书通知</h2><FeishuBotConfiguration onChanged={()=>setConnection(null)} onState={(settings,snapshot)=>setBot({settings,snapshot})}/><h3>固定测试消息</h3><p>{connection ? connection.ready ? '长连接接收已就绪' : '长连接接收尚未就绪' : checking?'正在查看接收状态…':'长连接接收状态见上方；可手动重新检查'}</p>{connection?.reason && <p>{connection.reason}</p>}<p role="status">{canSend?'HTTP 发送服务存活，可显式测试；接收连接状态单独显示。':'HTTP 发送尚未就绪；需保存发送开关并显式启动服务。'}</p><p className="muted small">只向已验证的本人通道发送固定测试消息，不包含学习内容，也不会开启全部提醒。</p><div className="button-row"><button className="quiet-button" disabled={!!busy} onClick={() => { void check(); }}>{busy === 'status' ? <LoaderCircle size={13} className="spin" /> : null}重新检查</button><button className="primary-button" disabled={!!busy || (!canSend&&result?.status!=='unknown')} onClick={() => { void test(); }}>{busy === 'test' ? <LoaderCircle size={13} className="spin" /> : <Send size={13} />}{busy === 'test' ? '正在核对发送结果…' : result?.status === 'unknown' ? '核对上次发送结果' : result ? '发送新的测试消息' : '发送测试消息'}</button></div>{requestError&&<div className="inline-error" role="alert">{requestError}{!draftReady&&<button onClick={()=>{void restoreRequest();}}>重试读取测试请求</button>}</div>}{error && <p className="inline-error" role="alert">无法查看通道状态：{error}</p>}{result && <div className={success ? 'settings-result' : 'inline-error'} role="status">{success ? <><Check size={13} />测试消息已发送</> : result.status === 'unknown' || result.status === 'sent' ? '发送结果未确认' : result.status === 'blocked' ? '未发送：连接条件未满足' : '发送失败'}<p>{result.reason}</p>{!success && (result.status === 'unknown' || result.status === 'sent') && <p>不会自动新发消息；核对时复用同一次请求编号。</p>}</div>}</section>;
}
