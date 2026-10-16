// Fixed-scope policy, durable receipts and HTTP sends; no CLI or history APIs.
import { createHash } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';
import { constants, closeSync, lstatSync, mkdirSync, openSync } from 'node:fs';
import { join } from 'node:path';
import https from 'node:https';

export const MAX_TEXT = 16000;
export const digest = value => createHash('sha256').update(value).digest('hex');
export const outcome = (status, reason, messageId = null) => ({ status, messageId, reason });
const plain = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const boundedText = text => typeof text === 'string' && text.trim() && !text.includes('\0') && Buffer.byteLength(text) <= MAX_TEXT;
export function parseConfig(value) {
  const keys = ['appId', 'ownerOpenId', 'chatId', 'receiveEnabled', 'deliveryEnabled', 'appSecret'];
  if (!plain(value) || Object.keys(value).some(key => !keys.includes(key))) throw Error('invalid_configuration');
  for (const [name, prefix] of [['appId', 'cli_'], ['ownerOpenId', 'ou_'], ['chatId', 'oc_']]) {
    if (typeof value[name] !== 'string' || !new RegExp(`^${prefix}[A-Za-z0-9_-]{1,160}$`).test(value[name])) throw Error('invalid_configuration');
  }
  if (!/^cli_[0-9a-fA-F]{16}$/.test(value.appId)) throw Error('invalid_configuration');
  for (const name of ['receiveEnabled', 'deliveryEnabled']) if (value[name] !== undefined && typeof value[name] !== 'boolean') throw Error('invalid_configuration');
  if (typeof value.appSecret !== 'string' || !value.appSecret.length || value.appSecret.length > 4096 || value.appSecret.includes('\0')) throw Error('missing_credentials');
  const config = { appId: value.appId, ownerOpenId: value.ownerOpenId, chatId: value.chatId,
    receiveEnabled: value.receiveEnabled ?? false, deliveryEnabled: value.deliveryEnabled ?? false };
  // Avoid accidental console/debug JSON serialization of the credential.
  Object.defineProperty(config, 'appSecret', { value: value.appSecret, enumerable: false });
  return Object.freeze(config);
}
export const binding = config => digest([config.appId, config.ownerOpenId, config.chatId].join('\0'));

export class Store {
  constructor(directory, config) {
    try { mkdirSync(directory, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
    const safe = (info, dir) => (dir ? info.isDirectory() : info.isFile()) && info.uid === process.getuid() && !(info.mode & 0o077) && (dir || info.nlink === 1);
    if (!safe(lstatSync(directory), true)) throw Error('unsafe_state_directory');
    const path = join(directory, `${binding(config)}.node.sqlite3`);
    try { const fd = openSync(path, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600); closeSync(fd); }
    catch (error) { if (error.code !== 'EEXIST') throw error; }
    if (!safe(lstatSync(path), false)) throw Error('unsafe_state_file');
    this.db = new DatabaseSync(path);
    this.db.exec(`PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;
      CREATE TABLE IF NOT EXISTS outbox(id TEXT PRIMARY KEY,body_hash TEXT NOT NULL,result TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS inbox(ticket TEXT PRIMARY KEY,message_hash TEXT UNIQUE NOT NULL,payload TEXT,acked INTEGER NOT NULL DEFAULT 0);`);
  }
  transaction(fn) {
    this.db.exec('BEGIN IMMEDIATE');
    try { const result = fn(); this.db.exec('COMMIT'); return result; }
    catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  receive(config, payload) {
    if (!config.receiveEnabled || !plain(payload)) return ['filtered', null];
    const { header, event } = payload;
    if (!plain(header) || !plain(event)) return ['filtered', null];
    const { sender, message } = event;
    if (header.event_type !== 'im.message.receive_v1' || !plain(sender) || !plain(message)
      || sender.sender_type !== 'user' || sender.sender_id?.open_id !== config.ownerOpenId
      || message.chat_id !== config.chatId || !['p2p', 'group'].includes(message.chat_type)
      || message.message_type !== 'text') return ['filtered', null];
    const ids = [header.event_id, message.message_id];
    if (ids.some(id => typeof id !== 'string' || !id.length || id.length > 256)
      || typeof message.content !== 'string' || Buffer.byteLength(message.content) > MAX_TEXT * 6 + 1024) return ['filtered', null];
    let text;
    try { text = JSON.parse(message.content)?.text; } catch { return ['filtered', null]; }
    if (!boundedText(text)) return ['filtered', null];
    const ticket = digest(header.event_id), messageHash = digest(message.message_id);
    const item = { type: 'inbound', ticket, messageId: message.message_id, chatId: config.chatId, senderOpenId: config.ownerOpenId, text };
    return this.transaction(() => {
      if (this.db.prepare('SELECT 1 FROM inbox WHERE ticket=? OR message_hash=?').get(ticket, messageHash)) return ['duplicate', null];
      if (this.db.prepare('SELECT COUNT(*) AS n FROM inbox WHERE acked=0').get().n >= 1000) throw Error('inbound_backlog_full');
      this.db.prepare('INSERT INTO inbox(ticket,message_hash,payload) VALUES(?,?,?)').run(ticket, messageHash, JSON.stringify(item));
      return ['accepted', item];
    });
  }
  pending() { return this.db.prepare('SELECT payload FROM inbox WHERE acked=0 ORDER BY rowid LIMIT 100').all().map(row => JSON.parse(row.payload)); }
  ack(ticket) {
    if (typeof ticket !== 'string' || !/^[a-f0-9]{64}$/.test(ticket)) return false;
    return this.transaction(() => this.db.prepare('UPDATE inbox SET acked=1,payload=NULL WHERE ticket=?').run(ticket).changes === 1);
  }
  async send(config, requestId, text, api) {
    if (!config.deliveryEnabled) return outcome('blocked', 'delivery_disabled');
    if (typeof requestId !== 'string' || !/^[A-Za-z0-9_-]{8,128}$/.test(requestId) || !boundedText(text)) return outcome('blocked', 'invalid_request');
    const key = digest(requestId), bodyHash = digest(text);
    const pending = outcome('unknown', 'delivery_unconfirmed_no_auto_retry');
    const previous = this.transaction(() => {
      const row = this.db.prepare('SELECT body_hash,result FROM outbox WHERE id=?').get(key);
      if (row) return row.body_hash === bodyHash ? JSON.parse(row.result) : outcome('blocked', 'request_id_conflict');
      this.db.prepare('INSERT INTO outbox VALUES(?,?,?)').run(key, bodyHash, JSON.stringify(pending));
      return null;
    });
    if (previous) return previous;
    let result;
    try {
      // 48 characters, deterministic and within the official API uuid bound.
      result = await api.send(config, text, digest(binding(config) + requestId).slice(0, 48));
      if (!plain(result) || !['sent', 'blocked', 'failed', 'unknown'].includes(result.status)
        || (result.status === 'sent' && (typeof result.messageId !== 'string' || !result.messageId.trim()))) result = pending;
    } catch { result = pending; }
    try { this.transaction(() => this.db.prepare('UPDATE outbox SET result=? WHERE id=?').run(JSON.stringify(result), key)); }
    catch { return pending; }
    return result;
  }
  close() { this.db.close(); }
}

export class HttpApi {
  #token = null;
  #expires = 0;
  #requests = new Set();
  post(path, data, token = null) {
    // No redirects, proxy env, arbitrary hosts or transport retries.
    return new Promise((resolve, reject) => {
      const body = Buffer.from(JSON.stringify(data));
      const headers = { 'Content-Type': 'application/json; charset=utf-8', 'Content-Length': body.length };
      if (token) headers.Authorization = `Bearer ${token}`;
      const request = https.request({ hostname: 'open.feishu.cn', port: 443, path: `/open-apis/${path}`, method: 'POST', headers,
        timeout: 12000, agent: false }, response => {
        let bytes = 0; const chunks = [];
        response.on('data', chunk => { bytes += chunk.length; if (bytes > 262144) request.destroy(Error('oversized_response')); else chunks.push(chunk); });
        response.on('error', reject);
        response.on('end', () => {
          if (response.statusCode !== 200) { reject(Error('http_rejected')); return; }
          try { const value = JSON.parse(Buffer.concat(chunks)); if (!plain(value)) throw Error('invalid_response'); resolve(value); }
          catch { reject(Error('invalid_response')); }
        });
      });
      const deadline = setTimeout(() => request.destroy(Error('http_deadline')), 12000);
      this.#requests.add(request);
      request.on('close', () => { clearTimeout(deadline); this.#requests.delete(request); });
      request.on('timeout', () => request.destroy(Error('http_timeout')));
      request.on('error', reject);
      request.end(body);
    });
  }
  abort() { for (const request of this.#requests) request.destroy(Error('shutdown')); }
  async send(config, text, messageUuid) {
    if (!this.#token || performance.now() >= this.#expires) {
      try {
        const value = await this.post('auth/v3/tenant_access_token/internal', { app_id: config.appId, app_secret: config.appSecret });
        if (value.code !== 0 || typeof value.tenant_access_token !== 'string' || !value.tenant_access_token
          || !Number.isInteger(value.expire) || value.expire <= 60) return outcome('blocked', 'bot_authentication_failed');
        this.#token = value.tenant_access_token; this.#expires = performance.now() + (value.expire - 60) * 1000;
      } catch { return outcome('blocked', 'bot_authentication_unavailable'); }
    }
    let response;
    try { response = await this.post('im/v1/messages?receive_id_type=chat_id', {
      receive_id: config.chatId, msg_type: 'text', content: JSON.stringify({ text }), uuid: messageUuid }, this.#token); }
    catch { return outcome('unknown', 'delivery_unconfirmed_no_auto_retry'); }
    if (response.code === 0 && typeof response.data?.message_id === 'string' && response.data.message_id.trim()) return outcome('sent', 'delivery_confirmed', response.data.message_id);
    if (Number.isInteger(response.code) && response.code !== 0) return outcome('failed', 'message_api_rejected');
    return outcome('unknown', 'delivery_unconfirmed_no_auto_retry');
  }
}
