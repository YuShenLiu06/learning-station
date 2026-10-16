// Trusted Node24 child; all config/credentials arrive over private stdin only.
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { realpathSync } from 'node:fs';
import { Store, HttpApi, parseConfig } from './adapter.mjs';

const SDK_VERSION = '1.76.0';
const noopLogger = Object.freeze({ error() {}, warn() {}, info() {}, debug() {}, trace() {} });
const keysEqual = (value, expected) => value && !Array.isArray(value) && typeof value === 'object'
  && Object.keys(value).sort().join() === [...expected].sort().join();

export function wireSdk(sdk, config, wire, onReceive) {
  const dispatcher = new sdk.EventDispatcher({ logger: noopLogger, loggerLevel: sdk.LoggerLevel.fatal }).register({
    // The official dispatcher flattens v2 header + event before this callback.
    'im.message.receive_v1': async data => onReceive({ header: { event_type: data.event_type, event_id: data.event_id },
      event: { sender: data.sender, message: data.message } }),
  });
  const client = new sdk.WSClient({ appId: config.appId, appSecret: config.appSecret, domain: sdk.Domain.Feishu,
    logger: noopLogger, loggerLevel: sdk.LoggerLevel.fatal, autoReconnect: true, handshakeTimeoutMs: 15000,
    onReady: () => wire.status('connected', 'websocket_handshake_confirmed'),
    onError: () => wire.status('failed', 'websocket_connection_failed'),
    onReconnecting: () => { wire.counts.reconnects++; wire.status('disconnected', 'websocket_disconnected'); wire.status('reconnecting', 'sdk_backoff'); },
    onReconnected: () => wire.status('connected', 'websocket_reconnected'),
  });
  return { client, dispatcher };
}

export class Wire {
  sequence = 0;
  state = 'starting';
  counts = { accepted: 0, filtered: 0, duplicate: 0, reconnects: 0 };
  constructor(output = process.stdout) { this.output = output; }
  emit(value) {
    const line = JSON.stringify({ protocol: 1, sequence: ++this.sequence, ...value }) + '\n';
    // A stuck parent is not an unbounded in-memory inbox. Unacked durable
    // messages survive process exit and are pulled by the next trusted worker.
    if (this.output.writableLength > 1024 * 1024) throw Error('parent_backpressure');
    this.output.write(line);
  }
  status(state = this.state, reason = null) {
    this.state = state; this.emit({ type: 'status', state, reason, counts: { ...this.counts }, at: Math.floor(Date.now() / 1000) });
  }
}

export function run(stateDir) {
  process.umask(0o077);
  const wire = new Wire();
  let store, config, client, monitor, closing = false, initialized = false, buffer = Buffer.alloc(0), queueDepth = 0;
  const api = new HttpApi();
  let chain = Promise.resolve();
  function shutdown(code = 0) {
    if (closing) return;
    closing = true;
    try { wire.status('stopping', 'shutdown_requested'); } catch {}
    clearInterval(monitor);
    try { client?.close({ force: true }); } catch {}
    api.abort();
    try { store?.close(); } catch {}
    try { wire.status('stopped', 'worker_exiting'); } catch {}
    // Flush the final safe status, then exit even if SDK endpoint discovery is in flight.
    process.stdout.end(() => process.exit(code));
    setTimeout(() => process.exit(code), 250).unref();
  }
  process.on('SIGTERM', () => shutdown());
  process.on('SIGINT', () => shutdown());
  process.on('uncaughtException', () => shutdown(1));
  process.on('unhandledRejection', () => { if (!closing) { wire.status('failed', 'runtime_failure'); shutdown(1); } });
  process.stdout.on('error', () => shutdown(1));

  async function command(value) {
    if (closing) return;
    if (!initialized) {
      if (!keysEqual(value, ['op', 'config']) || value.op !== 'init') throw Error('invalid_protocol');
      config = parseConfig(value.config);
      value.config.appSecret = undefined;
      if (process.versions.node.split('.')[0] !== '24') { wire.status('blocked', 'node_version_unsupported'); shutdown(2); return; }
      let sdk;
      if (config.receiveEnabled) {
        try {
          const require = createRequire(import.meta.url);
          if (require('@larksuiteoapi/node-sdk/package.json').version !== SDK_VERSION) { wire.status('blocked', 'sdk_version_unsupported'); shutdown(2); return; }
          sdk = require('@larksuiteoapi/node-sdk');
        } catch { wire.status('blocked', 'sdk_unavailable'); shutdown(2); return; }
      }
      store = new Store(stateDir, config); initialized = true;
      if (sdk) {
        const wired = wireSdk(sdk, config, wire, payload => {
          if (closing) throw Error('stopping');
          try { const [decision, item] = store.receive(config, payload); wire.counts[decision]++; if (item) wire.emit(item); else wire.status(); }
          catch { wire.status('failed', 'inbound_persistence_failed'); throw Error('inbound_persistence_failed'); }
        });
        client = wired.client;
        wire.status('connecting', 'configured');
        await client.start({ eventDispatcher: wired.dispatcher });
        monitor = setInterval(() => {
          if (closing) return;
          const status = client.getConnectionStatus();
          if (status.state === 'idle' && wire.state === 'connected') wire.status('disconnected', 'websocket_disconnected');
          else if (status.state === 'failed' && wire.state !== 'failed') wire.status('failed', 'websocket_connection_failed');
        }, 500);
      } else wire.status('receive_disabled', 'configured');
      return;
    }
    const op = value?.op;
    if (op === 'status' && keysEqual(value, ['op'])) wire.status();
    else if (op === 'pending' && keysEqual(value, ['op'])) { for (const item of store.pending()) wire.emit(item); }
    else if (op === 'ack' && keysEqual(value, ['op', 'ticket'])) wire.emit({ type: 'ack', ticket: value.ticket, accepted: store.ack(value.ticket) });
    else if (op === 'send' && keysEqual(value, ['op', 'requestId', 'text'])) wire.emit({ type: 'delivery', requestId: value.requestId, outcome: await store.send(config, value.requestId, value.text, api) });
    else if (op === 'stop' && keysEqual(value, ['op'])) shutdown();
    else wire.emit({ type: 'error', reason: 'unsupported_command' });
  }
  process.stdin.on('data', chunk => {
    if (closing) return;
    buffer = Buffer.concat([buffer, chunk]);
    let end;
    while ((end = buffer.indexOf(10)) >= 0) {
      if (end + 1 > 65536 || ++queueDepth > 128) { wire.status('failed', 'invalid_protocol'); shutdown(2); return; }
      const line = buffer.subarray(0, end); buffer = buffer.subarray(end + 1);
      chain = chain.then(async () => { await command(JSON.parse(line)); }).catch(() => {
        if (!closing) { wire.status('blocked', 'invalid_configuration_or_protocol'); shutdown(2); }
      }).finally(() => queueDepth--);
    }
    if (buffer.length > 65536) { wire.status('failed', 'invalid_protocol'); shutdown(2); }
  });
  process.stdin.on('end', () => {
    chain.finally(() => { if (buffer.length && !closing) { wire.status('failed', 'invalid_protocol'); shutdown(2); } else shutdown(); });
  });
}

if (process.argv[1] && realpathSync(resolve(process.argv[1])) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 4 || process.argv[2] !== '--state-dir') process.exit(2);
  run(process.argv[3]);
}
