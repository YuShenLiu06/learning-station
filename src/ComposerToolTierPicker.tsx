import { useEffect, useRef, useState } from 'react';
import { BookOpen, Check, ChevronUp, LoaderCircle, PenLine, ShieldBan, ShieldCheck, SquareTerminal } from 'lucide-react';
import { call } from './api';
import { normalizeToolAuthorizationTier, TIER_DESCRIPTIONS, TIER_LABELS, TOOL_AUTHORIZATION_TIERS } from './toolAuthorization';
import type { ToolAuthorizationTier } from './toolAuthorization';

const TIER_ICONS: Record<ToolAuthorizationTier, typeof ShieldCheck> = { off: ShieldBan, read: BookOpen, write: PenLine, bash: SquareTerminal };

export function ComposerToolTierPicker({ busy }: { busy: boolean }) {
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState<ToolAuthorizationTier | null>(null);
  const [pending, setPending] = useState<ToolAuthorizationTier | null>(null);
  const [error, setError] = useState('');
  const lock = useRef(false);
  async function load() {
    try {
      const value = await call<{ toolAuthorizationTier?: unknown }>('preferences_get');
      setSaved(normalizeToolAuthorizationTier(value.toolAuthorizationTier)); setError('');
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    }
  }
  useEffect(() => { void load(); }, []);
  useEffect(() => { if (busy) setOpen(false); }, [busy]);
  const current = saved ?? 'off';
  async function choose(tier: ToolAuthorizationTier) {
    if (lock.current || busy || saved === null) return;
    if (tier === current) { setOpen(false); return; }
    lock.current = true; setPending(tier); setError('');
    try {
      const value = await call<{ toolAuthorizationTier?: unknown }>('preferences_save', { toolAuthorizationTier: tier });
      setSaved(normalizeToolAuthorizationTier(value.toolAuthorizationTier));
      setOpen(false);
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      lock.current = false; setPending(null);
    }
  }
  const Icon = TIER_ICONS[current];
  return <div className="composer-tier-picker">
    <button type="button" className="tier-pill" aria-expanded={open} aria-haspopup="menu" disabled={busy}
      title={busy ? '请先停止生成，再变更工具档位' : `模型工具档位：${TIER_LABELS[current]}。${TIER_DESCRIPTIONS[current]}`}
      onClick={() => { setError(''); setOpen(!open); }}>
      {pending ? <LoaderCircle size={11} className="spin" /> : <Icon size={11} />}{TIER_LABELS[current]}<ChevronUp size={10} />
    </button>
    {open && <>
      <div className="tier-backdrop" aria-hidden="true" onClick={() => setOpen(false)} />
      <div className="tier-menu" role="menu" aria-label="模型工具档位">
        {TOOL_AUTHORIZATION_TIERS.map(tier => { const TierIcon = TIER_ICONS[tier]; return (
          <button key={tier} type="button" role="menuitemradio" aria-checked={tier === current} className={tier === current ? 'selected' : ''} disabled={!!pending}
            onClick={() => { void choose(tier); }}>
            <TierIcon size={13} /><span className="tier-option-text"><strong>{TIER_LABELS[tier]}</strong><small>{TIER_DESCRIPTIONS[tier]}</small></span>{tier === current && <Check size={12} />}
          </button>); })}
        <p className="muted small">档位仅本人选择，默认关闭；生成进行中不能变更。变更后新请求按此档位注册工具。</p>
        {error && <div className="inline-error" role="alert">{error}{saved === null && <button onClick={() => { void load(); }}>重试读取</button>}</div>}
      </div>
    </>}
  </div>;
}
