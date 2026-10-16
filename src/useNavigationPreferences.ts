import { useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import type { UiPreferences } from './types';
export function useNavigationPreferences() {
  const [collapsed, setCollapsedValue] = useState(false); const [expandedIds, setExpandedValue] = useState<string[]>([]); const [error, setError] = useState('');
  const state = useRef({ collapsed: false, expanded: [] as string[], editedCollapsed: false, editedExpanded: false, loaded: !nativeAvailable, savedCollapsed: false, savedExpanded: '[]' });
  const pendingAncestors = useRef(new Set<string>());
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  async function load() { try { const value = await call<UiPreferences>('preferences_get'); const record = state.current; record.savedCollapsed = value.sidebarCollapsed === true; record.savedExpanded = JSON.stringify(value.expandedNodeIds || []); record.loaded = true; if (!record.editedCollapsed) { record.collapsed = record.savedCollapsed; setCollapsedValue(record.collapsed); } if (!record.editedExpanded) { record.expanded = value.expandedNodeIds || []; setExpandedValue(record.expanded); } if (pendingAncestors.current.size) { const merged = [...new Set([...record.expanded, ...pendingAncestors.current])]; pendingAncestors.current.clear(); if (JSON.stringify(merged) !== JSON.stringify(record.expanded)) { record.expanded = merged; record.editedExpanded = true; setExpandedValue(merged); } } setError(''); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } }
  useEffect(() => { if (nativeAvailable) void load(); }, []);
  async function flush() {
    const record = state.current; if (!nativeAvailable || !record.editedCollapsed && !record.editedExpanded) return;
    if (!record.loaded) throw new Error('导航偏好尚未读取成功，当前展开状态仍保留。');
    const collapsedValue = record.collapsed; const expandedValue = [...record.expanded]; const changeCollapsed = record.editedCollapsed; const changeExpanded = record.editedExpanded;
    const next = queue.current.catch(() => undefined).then(async () => { const payload: Record<string, unknown> = {}; if (changeCollapsed && record.savedCollapsed !== collapsedValue) payload.sidebarCollapsed = collapsedValue; if (changeExpanded && record.savedExpanded !== JSON.stringify(expandedValue)) payload.expandedNodeIds = expandedValue; if (!Object.keys(payload).length) return; await call('preferences_save', payload); if (payload.sidebarCollapsed !== undefined) record.savedCollapsed = collapsedValue; if (payload.expandedNodeIds !== undefined) record.savedExpanded = JSON.stringify(expandedValue); }); queue.current = next;
    try { await next; setError(''); } catch (error) { setError(error instanceof Error ? error.message : String(error)); throw error; }
  }
  useEffect(() => { if (!state.current.loaded) return; const timer = setTimeout(() => { void flush().catch(() => undefined); }, 220); return () => clearTimeout(timer); }, [collapsed, expandedIds]);
  return { collapsed, expandedIds, error, flush, retry: load, ensureExpanded(ids: string[]) { if (!ids.length) return; if (!state.current.loaded) { ids.forEach(id => pendingAncestors.current.add(id)); return; } const merged = [...new Set([...state.current.expanded, ...ids])]; if (JSON.stringify(merged) === JSON.stringify(state.current.expanded)) return; state.current.expanded = merged; state.current.editedExpanded = true; setExpandedValue(merged); }, setCollapsed(value: boolean) { state.current.collapsed = value; state.current.editedCollapsed = true; setCollapsedValue(value); }, setExpandedIds(value: string[]) { state.current.expanded = [...new Set(value)]; state.current.editedExpanded = true; setExpandedValue(state.current.expanded); } };
}
