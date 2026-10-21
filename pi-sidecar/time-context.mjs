/** Trusted, per-logical-request clock; never derived from course text or model arguments. */
export function formatInstant(instant, timeZone) {
  const date = new Date(instant);
  if (!Number.isFinite(date.getTime())) throw new Error('TIME_CONTEXT_INVALID');
  if (!timeZone) return { utcOffset: null, localDateTime: null };
  const parts = new Intl.DateTimeFormat('en-CA', { timeZone, year:'numeric', month:'2-digit', day:'2-digit', hour:'2-digit', minute:'2-digit', second:'2-digit', hourCycle:'h23', timeZoneName:'longOffset' }).formatToParts(date);
  const get = name => parts.find(part => part.type === name)?.value;
  const zone = get('timeZoneName');
  const utcOffset = zone === 'GMT' || zone === 'UTC' ? '+00:00' : zone?.replace(/^GMT/u, '');
  if (!/^[+-]\d{2}:\d{2}$/u.test(utcOffset ?? '')) throw new Error('TIME_CONTEXT_INVALID');
  return { utcOffset, localDateTime: `${get('year')}-${get('month')}-${get('day')}T${get('hour')}:${get('minute')}:${get('second')}${date.getUTCMilliseconds() ? '.'+String(date.getUTCMilliseconds()).padStart(3,'0') : ''}${utcOffset}` };
}
export function resolveTimeContext(input) {
  const source = input === undefined ? { referenceTime: new Date().toISOString(), timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone ?? null } : input;
  if (!source || typeof source !== 'object' || Array.isArray(source) || Object.keys(source).some(key => !['referenceTime','timeZone'].includes(key)) || typeof source.referenceTime !== 'string' || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,3})?Z$/u.test(source.referenceTime) || !Number.isFinite(Date.parse(source.referenceTime)) || !(source.timeZone === null || typeof source.timeZone === 'string' && source.timeZone.length > 0 && source.timeZone.length <= 100)) throw new Error('TIME_CONTEXT_INVALID');
  const local = formatInstant(source.referenceTime, source.timeZone);
  return Object.freeze({ referenceTime: source.referenceTime, timeZone: source.timeZone, ...local });
}
export function timeSystemContext(context) {
  return `当前时间：${context.localDateTime ?? context.referenceTime}${context.timeZone ? `（${context.timeZone}）` : ''}`;
}
export function freezeTodoTime(args, context) {
  if (args.relativeMinutes === undefined) return args;
  if (!Number.isInteger(args.relativeMinutes) || args.relativeMinutes < 1 || args.relativeMinutes > 525600 || !context) throw new Error('TOOL_PROTOCOL');
  const target = new Date(Date.parse(context.referenceTime) + args.relativeMinutes * 60000);
  if (args.dueAt !== undefined && Date.parse(args.dueAt) !== target.getTime()) throw new Error('TOOL_PROTOCOL');
  const local = formatInstant(target, context.timeZone);
  return { ...args, dueAt: local.localDateTime ?? target.toISOString().replace('.000Z','Z') };
}
