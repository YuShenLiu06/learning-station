import { useEffect, useRef, useState } from 'react';
import { call } from './api';
import { normalizeToolAuthorizationTier, TIER_DESCRIPTIONS, TIER_LABELS, TOOL_AUTHORIZATION_TIERS } from './toolAuthorization';
import type { ToolAuthorizationTier } from './toolAuthorization';

export function ToolAuthorizationSettings({ blocked }: { blocked: boolean }) {
  const [saved, setSaved] = useState<ToolAuthorizationTier | null>(null);
  const [choice, setChoice] = useState<ToolAuthorizationTier>('off');
  const [error, setError] = useState('');
  const [result, setResult] = useState('');
  const [busy, setBusy] = useState(false);
  const lock = useRef(false);
  async function load() {
    try {
      const value = await call<{ toolAuthorizationTier?: unknown }>('preferences_get');
      const tier = normalizeToolAuthorizationTier(value.toolAuthorizationTier);
      setSaved(tier); setChoice(tier); setError('');
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    }
  }
  useEffect(() => { void load(); }, []);
  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (lock.current || blocked || saved === null) return;
    lock.current = true; setBusy(true); setError(''); setResult('');
    try {
      const value = await call<{ toolAuthorizationTier?: unknown }>('preferences_save', { toolAuthorizationTier: choice });
      const tier = normalizeToolAuthorizationTier(value.toolAuthorizationTier);
      setSaved(tier); setChoice(tier);
      setResult(tier === 'off' ? '已保存：工具授权保持关闭。' : `已保存：${TIER_LABELS[tier]}。新请求按此档位注册工具。`);
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      lock.current = false; setBusy(false);
    }
  }
  return <section className="tool-authorization-settings" aria-label="工具授权档位"><h2 className="settings-section-title">工具授权</h2>
    <p className="muted small">由本人在此选择模型可用的文件与命令档位；默认关闭，不会替你开启。生成进行中不能变更档位。</p>
    <form onSubmit={event => { void save(event); }}>
      <fieldset className="provider-fields" disabled={!saved || busy || blocked}>
        {TOOL_AUTHORIZATION_TIERS.map(tier => <label key={tier} className="tier-option"><input type="radio" name="tool-authorization-tier" value={tier} checked={choice === tier} onChange={() => setChoice(tier)} />{TIER_LABELS[tier]}<span className="label-note">{TIER_DESCRIPTIONS[tier]}</span></label>)}
      </fieldset>
      {blocked && <p className="muted small">正在生成；请先停止生成再变更授权档位。</p>}
      {error && <div className="inline-error" role="alert">{error}{saved === null && <button onClick={() => { void load(); }}>重试读取</button>}</div>}
      {result && <p className="settings-result" role="status">{result}</p>}
      <div className="dialog-footer"><button className="quiet-button" type="submit" disabled={!saved || busy || blocked}>{busy ? '正在保存…' : '保存授权档位'}</button></div>
    </form>
  </section>;
}
