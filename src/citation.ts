import { StationError } from './types.ts';
import type { CitationLocator } from './types.ts';
export type LocatedCitation = {
    selection: string;
    locator: CitationLocator;
};
const conflict = () => new StationError('此选段无法精确对应原文，请在原文中重新选择。', 'CONTEXT_CONFLICT');
export function validateCitation(selection: string, locator?: CitationLocator) {
    if (!selection || Array.from(selection).length > 8000 || new TextEncoder().encode(selection).length > 30000)
        throw new StationError('引用最多 8000 字符、30000 字节，请缩小选段。', 'LIMIT_EXCEEDED');
    if (locator && (!Number.isSafeInteger(locator.startChar) || !Number.isSafeInteger(locator.endChar) || locator.startChar < 0 || locator.endChar - locator.startChar !== Array.from(selection).length))
        throw conflict();
}
export function locateCitation(content: string, selection: string, start?: number): LocatedCitation {
    if (start === undefined) {
        start = content.indexOf(selection);
        if (start !== content.lastIndexOf(selection))
            throw conflict();
    }
    if (start < 0 || content.slice(start, start + selection.length) !== selection)
        throw conflict();
    const locator = { startChar: Array.from(content.slice(0, start)).length, endChar: Array.from(content.slice(0, start + selection.length)).length };
    validateCitation(selection, locator);
    return { selection, locator };
}
export const cloneCitation = <T extends {
    locator?: CitationLocator;
}>(item: T): T => ({ ...item, ...(item.locator ? { locator: { ...item.locator } } : {}) });
export function readLocator(value: unknown, selection: string): CitationLocator | undefined {
    if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).some(key => !['startChar', 'endChar'].includes(key)))
        throw conflict();
    const locator = value as CitationLocator;
    validateCitation(selection, locator);
    return { ...locator };
}
