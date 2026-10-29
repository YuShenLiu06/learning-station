import { useRef, useState } from 'react';
import { call } from './api';
import { citationForAsset } from './assetReferences';
import type { CourseAsset } from './types';

export function clipboardImage(data: DataTransfer | null): File | undefined {
  return Array.from(data?.items || []).find(item => item.kind === 'file' && item.type.startsWith('image/'))?.getAsFile() || undefined;
}

export function insertPastedImage(current: string, original: string, from: number, to: number, citation: string) {
  // Never replace text typed while the import was in flight.
  return current === original ? current.slice(0, from) + citation + current.slice(to) : `${current}${current ? '\n' : ''}${citation}`;
}

type Attempt = { file: File; courseId: string; requestId: string; insert: (citation: string) => void };
export function useImagePaste() {
  const attempt = useRef<Attempt | null>(null);
  const working = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function run(value: Attempt) {
    if (working.current) return;
    working.current = true; setBusy(true); setError('');
    try {
      const asset = await call<CourseAsset>('asset_import', { courseId: value.courseId, fileName: value.file.name, bytes: Array.from(new Uint8Array(await value.file.arrayBuffer())), requestId: value.requestId });
      value.insert(citationForAsset(asset)); attempt.current = null;
    } catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { working.current = false; setBusy(false); }
  }
  return {
    busy,
    paste(file: File, courseId: string | undefined, insert: Attempt['insert']) {
      if (working.current || attempt.current) { setError('上一张图片尚未导入完成，请先重试或取消。'); return false; }
      if (!courseId) { setError('请先打开课程中的笔记或对话，再粘贴图片。'); return false; }
      if (!['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(file.type)) { setError('支持粘贴 PNG、JPEG、GIF 或 WebP 图片。'); return false; }
      if (file.size > 5 * 1024 * 1024) { setError('单张图片最多 5MiB。'); return false; }
      const value = { file, courseId, requestId: crypto.randomUUID(), insert }; attempt.current = value; void run(value);
      return true;
    },
    status: busy ? <p className="muted small" role="status">正在保存粘贴的图片…</p> : error ? <div className="inline-error" role="alert">{error}{attempt.current && <><button type="button" onClick={() => { if (attempt.current) void run(attempt.current); }}>重试图片导入</button><button type="button" onClick={() => { attempt.current = null; setError(''); }}>取消</button></>}</div> : null,
  };
}
