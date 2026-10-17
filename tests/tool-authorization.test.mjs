import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('tool authorization tiers are ordered, labeled and normalized closed', async () => {
  const filename = new URL(`./.tool-authorization-${process.pid}.mjs`, import.meta.url);
  await build({ stdin: { contents: "export {TOOL_AUTHORIZATION_TIERS,isToolAuthorizationTier,normalizeToolAuthorizationTier} from './src/toolAuthorization';", resolveDir: process.cwd(), loader: 'tsx' }, outfile: filename.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm' });
  try {
    const { TOOL_AUTHORIZATION_TIERS, isToolAuthorizationTier, normalizeToolAuthorizationTier } = await import(filename.href);
    assert.deepEqual(TOOL_AUTHORIZATION_TIERS, ['off', 'read', 'write', 'bash']);
    assert.equal(TOOL_AUTHORIZATION_TIERS[0], 'off');
    assert.equal(isToolAuthorizationTier('bash'), true);
    assert.equal(isToolAuthorizationTier('shell'), false);
    for (const invalid of [undefined, null, 'shell', 'READ', '', 3]) {
      assert.equal(normalizeToolAuthorizationTier(invalid), 'off', `${String(invalid)} must normalize to off`);
    }
    assert.equal(normalizeToolAuthorizationTier('read'), 'read');
  } finally { await unlink(filename).catch(() => {}); }
});

test('settings panel keeps tier off by default, saves the explicit user choice and surfaces busy boundaries', async () => {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://127.0.0.1/' });
  Object.assign(globalThis, { window: dom.window, document: dom.window.document, isTauri: true, IS_REACT_ACT_ENVIRONMENT: true });
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: dom.window.navigator });
  for (const name of ['HTMLElement', 'Element', 'Node', 'Event', 'HTMLInputElement']) globalThis[name] = dom.window[name];
  const { mockIPC, clearMocks } = await import('@tauri-apps/api/mocks');
  let prefs = { fontSize: 16 };
  const writes = [];
  let busyOnce = false;
  mockIPC(async (command, args = {}) => {
    if (command === 'get_ui_preferences') return structuredClone({ ...prefs, toolAuthorizationTier: prefs.toolAuthorizationTier ?? 'off' });
    if (command === 'save_ui_preferences') {
      if (busyOnce) { busyOnce = false; throw { code: 'BUSY', message: '生成中不能变更工具授权档位，请先停止并核对未决工具回执。' }; }
      writes.push(structuredClone(args)); prefs = { ...prefs, ...args }; return structuredClone({ ...prefs, toolAuthorizationTier: prefs.toolAuthorizationTier ?? 'off' });
    }
    throw Error('Unexpected mock IPC ' + command);
  });
  const filename = new URL(`./.tool-authorization-ui-${process.pid}.mjs`, import.meta.url);
  await build({ stdin: { contents: "export {ToolAuthorizationSettings} from './src/ToolAuthorizationSettings';", resolveDir: process.cwd(), loader: 'tsx' }, outfile: filename.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm', jsx: 'automatic', define: { 'import.meta.env.DEV': 'false' } });
  const React = await import('react');
  const { createRoot } = await import('react-dom/client');
  const { ToolAuthorizationSettings } = await import(filename.href);
  const root = createRoot(document.getElementById('root'));
  const act = React.act;
  const selected = () => document.querySelector('input[name="tool-authorization-tier"]:checked')?.value;
  try {
    await act(async () => root.render(React.createElement(ToolAuthorizationSettings, { blocked: false, sessionId: 'session_prefs' })));
    assert.equal(selected(), 'off', 'a missing stored tier renders as off');
    assert.match(document.body.textContent, /默认/);
    await act(async () => { (document.querySelector('input[name="tool-authorization-tier"][value="bash"]')).click(); });
    await act(async () => { document.querySelector('button[type="submit"]').click(); });
    assert.deepEqual(writes, [{ toolAuthorizationTier: 'bash' }]);
    assert.match(document.body.textContent, /已保存/);
    // A busy boundary error stays visible and does not fake success.
    busyOnce = true;
    await act(async () => { (document.querySelector('input[name="tool-authorization-tier"][value="read"]')).click(); });
    await act(async () => { document.querySelector('button[type="submit"]').click(); });
    assert.equal(writes.length, 1, 'rejected save writes nothing');
    assert.match(document.body.textContent, /生成中不能变更工具授权档位/);
    assert.equal(selected(), 'read', 'the attempted choice stays in the form');
  } finally {
    await act(async () => root.unmount());
    clearMocks(); dom.window.close(); await unlink(filename).catch(() => {});
  }
});

test('receipt audit entry is read-only and never renders command text', async () => {
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://127.0.0.1/' });
  Object.assign(globalThis, { window: dom.window, document: dom.window.document, isTauri: true, IS_REACT_ACT_ENVIRONMENT: true });
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: dom.window.navigator });
  for (const name of ['HTMLElement', 'Element', 'Node', 'Event', 'HTMLInputElement']) globalThis[name] = dom.window[name];
  const { mockIPC, clearMocks } = await import('@tauri-apps/api/mocks');
  // The decoy command/content fields prove the UI only renders whitelisted summary fields.
  const ledger = { truncated: false, receipts: [
    { requestId: 'pi_tool_lease00000000000000000000000001', tool: 'write', status: 'persisted', createdAt: '2026-10-17T08:30:00.000Z', path: 'notes/draft.md', command: 'echo SHOULD_NOT_RENDER', content: 'SHOULD_NOT_RENDER_EITHER' },
    { requestId: 'pi_tool_lease00000000000000000000000002', tool: 'bash', status: 'timed-out', createdAt: '2026-10-17T08:31:00.000Z', command: 'sleep 5; echo SHOULD_NOT_RENDER' }
  ] };
  mockIPC(async (command, args = {}) => {
    if (command === 'get_ui_preferences') return { fontSize: 16, toolAuthorizationTier: 'bash' };
    if (command === 'list_tool_receipts') { assert.equal(args.sessionId, 'session_ui'); return structuredClone(ledger); }
    throw Error('Unexpected mock IPC ' + command);
  });
  const filename = new URL(`./.tool-receipts-ui-${process.pid}.mjs`, import.meta.url);
  await build({ stdin: { contents: "export {ToolAuthorizationSettings} from './src/ToolAuthorizationSettings';", resolveDir: process.cwd(), loader: 'tsx' }, outfile: filename.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm', jsx: 'automatic', define: { 'import.meta.env.DEV': 'false' } });
  const React = await import('react');
  const { createRoot } = await import('react-dom/client');
  const { ToolAuthorizationSettings } = await import(filename.href);
  const root = createRoot(document.getElementById('root'));
  const act = React.act;
  try {
    await act(async () => root.render(React.createElement(ToolAuthorizationSettings, { blocked: false, sessionId: 'session_ui' })));
    const toggle = document.querySelector('button[aria-expanded]');
    assert.equal(toggle.textContent, '查看本会话工具回执');
    await act(async () => { toggle.click(); });
    assert.equal(document.querySelector('button[aria-expanded]').getAttribute('aria-expanded'), 'true');
    const text = document.body.textContent;
    assert.match(text, /notes\/draft\.md/, 'write receipts show the relative path only');
    assert.match(text, /已写入/);
    assert.match(text, /已超时/);
    assert.doesNotMatch(text, /SHOULD_NOT_RENDER/, 'command text and file content must never render');
    const items = document.querySelectorAll('ul[aria-label="本会话工具回执"] li');
    assert.equal(items.length, 2);
    assert.equal(document.querySelectorAll('ul[aria-label="本会话工具回执"] button').length, 0, 'the audit list is read-only');
    // Collapsing only hides the list; nothing is written either way.
    await act(async () => { document.querySelector('button[aria-expanded]').click(); });
    assert.equal(document.querySelector('ul[aria-label="本会话工具回执"]'), null);
  } finally {
    await act(async () => root.unmount());
    clearMocks(); dom.window.close(); await unlink(filename).catch(() => {});
  }
});
