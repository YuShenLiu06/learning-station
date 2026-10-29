import { getBuiltinModels, getBuiltinProviders } from '@earendil-works/pi-ai/providers/all';

const MAX_IMAGE = 5 * 1024 * 1024;
export function supportsImageInput(settings) {
  if (typeof settings.supportsImages === 'boolean') return settings.supportsImages;
  const matches = getBuiltinProviders().flatMap(getBuiltinModels).filter(model => model.id === settings.model);
  return matches.length > 0 && matches.every(model => model.input.includes('image'));
}

// Rust owns source authorization; this boundary also rejects malformed or oversized IPC.
export function validateImages(value = []) {
  if (!Array.isArray(value) || value.length > 4) throw Object.assign(new Error('INVALID_INPUT'), { code: 'INVALID_INPUT' });
  for (const image of value) {
    if (image?.type !== 'image' || !['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(image.mimeType)
      || typeof image.data !== 'string' || !image.data.length || image.data.length > Math.ceil(MAX_IMAGE / 3) * 4
      || image.data.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/u.test(image.data)
      || image.sourceId !== undefined && (typeof image.sourceId !== 'string' || !/^asset_[a-f0-9]{32}$/u.test(image.sourceId))
      || Buffer.byteLength(image.data, 'base64') > MAX_IMAGE) {
      throw Object.assign(new Error('INVALID_INPUT'), { code: 'INVALID_INPUT' });
    }
  }
  return value.map(({ data, mimeType, sourceId }) => ({ type: 'image', data, mimeType, ...(sourceId ? {sourceId} : {}) }));
}

export function imageSourceText(question, images) {
  const sources = images.flatMap((image, index) => image.sourceId ? [`${index + 1}: asset:${image.sourceId}`] : []);
  return sources.length ? `${question}\n\n图片顺序与来源：\n${sources.join('\n')}` : question;
}

// Applies to old transcript images and compaction as well as the new user turn.
// Pi's OpenAI user-message serializer does not enforce model.input itself.
export function imageCompatibleStream(stream) {
  return (model, context, options) => stream(model, model.input?.includes('image') ? context : {
    ...context,
    messages: context.messages.map(message => !Array.isArray(message.content) ? message : {
      ...message, content: message.content.filter(part => part.type !== 'image'),
    }),
  }, options);
}
