import { isTauri } from '@tauri-apps/api/core';
import { StationError } from './types';
import { nativeCall } from './native';
import type { Bootstrap, Course, LearningSession, ModelSettings, Section } from './types';

export const isDemo = import.meta.env.DEV && new URLSearchParams(location.search).get('demo') === '1';
export const nativeAvailable = isTauri();
const defaultModel: ModelSettings = { configured: false, provider: 'custom', protocol: 'openai-completions', usageGate: '', baseUrl: '', model: '', harness: 'pi-agent-core', credentialStored: false };
export const sampleBody = `# The quiet power of a daily habit\n\nA small action, repeated consistently, can have a remarkable effect over time. Yet when we try to build a new habit, we often focus on the outcome rather than the process.\n\n## Read the passage\n\nWhen Maya decided to improve her English, she set herself an ambitious goal: read a novel every week. For the first few days, enthusiasm carried her forward. By the second week, however, the habit had begun to feel like a burden.\n\nInstead of giving up altogether, she made a small change. She would read just one page after breakfast. The task was so easy that she could do it even on a busy morning. Before long, one page became two, and then three.\n\n**What mattered was not how much she read on any particular day, but whether she kept returning to the book.**\n\nResearchers call this approach “starting small”. By reducing the effort required to begin, we make it easier to repeat an action. A familiar cue — such as finishing breakfast — helps the action become part of an existing routine.\n\n## Think about the language\n\n> Instead of giving up altogether, she made a small change.\n\nWhat does “instead of” tell us about the relationship between the two actions? How would the meaning change if we used “because of”?\n\n## A question to carry with you\n\nWhat small action could make your own learning easier to return to tomorrow?`;
let demoSection: Section = { id: 'sample-section', courseId: 'sample-course', title: '01 · The quiet power of a daily habit', body: sampleBody, version: '1', sourcePath: '示例课程 / daily-habit.md' };
const demoCourses: Course[] = [{ id: 'sample-course', title: '英语阅读 · 示例课程', sections: [{ id: demoSection.id, title: demoSection.title }] }];
let demoSession: LearningSession = { id: 'sample-session', sectionId: demoSection.id, messages: [], exercises: [], mistakes: [], cards: [] };

async function demoCall<T>(command: string, payload: Record<string, unknown>): Promise<T> {
  switch (command) {
    case 'bootstrap': return { courses: demoCourses, lastSectionId: demoSection.id, model: defaultModel, tasks: [], notices: [], dataPath: '开发预览 · 数据仅在内存' } as T;
    case 'section_open': return { ...demoSection } as T;
    case 'context_load': return { text: '', manifest: [], warnings: ['开发预览未连接本机上下文服务'], truncated: false } as T;
    case 'exploration_open': return { session: { ...demoSession, id: 'demo-independent', sectionId: '', messages: [], sources: [] }, context: { text: '', manifest: [], warnings: [], truncated: false } } as T;
    case 'activity_refresh': return { tasks: [], notices: [] } as T;
    case 'session_open': return { ...demoSession } as T;
    case 'section_save': demoSection = { ...demoSection, body: String(payload.body), version: String(Number(demoSection.version) + 1) }; return { ...demoSection } as T;
    case 'section_import': demoSection = { ...demoSection, title: String(payload.title), body: String(payload.body) }; demoCourses[0].sections[0].title = demoSection.title; return { ...demoSection } as T;
    case 'review_list': return [] as T;
    case 'model_get': return defaultModel as T;
    case 'card_create': { const card = { id: crypto.randomUUID(), ...payload }; demoSession = { ...demoSession, cards: [...demoSession.cards, card as never] }; return card as T; }
    default: throw new StationError('开发预览没有连接本机服务。请在桌面应用中完成此操作。', 'DEMO_UNAVAILABLE');
  }
}

export async function call<T>(command: string, payload: Record<string, unknown> = {}): Promise<T> {
  if (isDemo) return demoCall<T>(command, payload);
  if (!nativeAvailable) throw new StationError('尚未连接学习站本机服务。请从桌面应用打开，或重新连接。', 'IPC_UNAVAILABLE');
  try {
    return await nativeCall(command, payload) as T;
  } catch (error) {
    if (typeof error === 'object' && error && 'message' in error) {
      const value = error as { message: string; code?: string; details?: unknown };
      throw new StationError(value.message, value.code, value.details);
    }
    throw new StationError(typeof error === 'string' ? error : '本机服务暂时没有响应，请重试。');
  }
}
export const emptyBootstrap: Bootstrap = { courses: [], model: defaultModel, tasks: [], notices: [] };

export function recordMetric(name: 'shell-interactive' | 'section-ready' | 'editor-ready' | 'page-switch' | 'ime-commit' | 'stream-input', valueMs: number) { if (nativeAvailable) void call('metric', { name, valueMs }).catch(() => undefined); }
