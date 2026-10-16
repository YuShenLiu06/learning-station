import { useEffect, useState } from 'react';
import { Check, LoaderCircle, Send } from 'lucide-react';
import { call } from './api';
import { feishuRequestId, feishuTestResult } from './feishuPolicy';
import type { FeishuTestResult as TestResult } from './feishuPolicy';
const attempt: { id: string; result: TestResult | null; pending: boolean } = { id: '', result: null, pending: false };
const attemptListeners = new Set<() => void>();
const publishAttempt = () => attemptListeners.forEach(listener => listener());
export function FeishuSettings() {
  const [connection, setConnection] = useState<{ ready: boolean; reason: string } | null>(null);
  const [result, setResult] = useState<TestResult | null>(attempt.result);
  const [checking, setChecking] = useState(false);
  const [testPending, setTestPending] = useState(attempt.pending);
  const busy = testPending ? 'test' : checking ? 'status' : '';
  const [error, setError] = useState('');
  async function check() { setChecking(true); setError(''); try { setConnection(await call('feishu_status')); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } finally { setChecking(false); } }
  useEffect(() => { const sync = () => { setTestPending(attempt.pending); setResult(attempt.result); }; attemptListeners.add(sync); sync(); return () => { attemptListeners.delete(sync); }; }, []);
  useEffect(() => { void check(); }, []);
  async function test() {
    if (busy || attempt.pending || !connection?.ready) return;
    attempt.pending = true;
    attempt.id = feishuRequestId(attempt.id, attempt.result, crypto.randomUUID());
    publishAttempt(); setError('');
    try { const value = feishuTestResult(await call<TestResult>('feishu_test', { requestId: attempt.id })); attempt.result = value; setResult(value); }
    catch { const value: TestResult = { status: 'unknown', messageId: null, reason: '发送结果尚未确认；请先核对这次结果，避免重复发送。' }; attempt.result = value; setResult(value); }
    finally { attempt.pending = false; publishAttempt(); }
  }
  const success = result?.status === 'sent' && !!result.messageId;
  return <section className="feishu-settings" aria-label="飞书通知设置"><h2>飞书通知</h2><p>{connection ? connection.ready ? '本人通知通道已就绪' : '通知通道尚未就绪' : '正在查看连接状态…'}</p>{connection?.reason && <p>{connection.reason}</p>}<p className="muted small">只向已验证的本人通道发送固定测试消息，不包含学习内容，也不会开启全部提醒。</p><div className="button-row"><button className="quiet-button" disabled={!!busy} onClick={() => { void check(); }}>{busy === 'status' ? <LoaderCircle size={13} className="spin" /> : null}重新检查</button><button className="primary-button" disabled={!!busy || !connection?.ready} onClick={() => { void test(); }}>{busy === 'test' ? <LoaderCircle size={13} className="spin" /> : <Send size={13} />}{busy === 'test' ? '正在核对发送结果…' : result?.status === 'unknown' ? '核对上次发送结果' : result ? '发送新的测试消息' : '发送测试消息'}</button></div>{error && <p className="inline-error" role="alert">无法查看通道状态：{error}</p>}{result && <div className={success ? 'settings-result' : 'inline-error'} role="status">{success ? <><Check size={13} />测试消息已发送</> : result.status === 'unknown' || result.status === 'sent' ? '发送结果未确认' : result.status === 'blocked' ? '未发送：连接条件未满足' : '发送失败'}<p>{result.reason}</p>{!success && (result.status === 'unknown' || result.status === 'sent') && <p>不会自动新发消息；核对时复用同一次请求编号。</p>}</div>}</section>;
}
