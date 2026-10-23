import { marked, type Token } from 'marked';
import { sanitizeMarkdown } from './sanitize';
import { StationError } from './types';
import { locateCitation, type LocatedCitation } from './citation';
const conflict = () => new StationError('此选段无法精确对应原文，请在原文中重新选择。', 'CONTEXT_CONFLICT');
const plain = (raw: string) => { const el = document.createElement('div'); el.innerHTML = sanitizeMarkdown(raw); return el.textContent || ''; };
const compact = (text: string) => text.replace(/\s/gu, '');
// Map visible token leaves, skipping Markdown destinations and syntax. The complete
// visible projection must match the actual DOM; unsupported rich content fails closed.
export function renderedCitation(body: HTMLElement, content: string, selected: Selection | null): LocatedCitation | undefined {
    if (!selected || selected.isCollapsed || !selected.rangeCount)
        return;
    if (selected.rangeCount !== 1 || !body.contains(selected.anchorNode) || !body.contains(selected.focusNode))
        throw new StationError('请只选同一条回答或同一材料的正文，不能跨消息或选入工具记录。', 'OUT_OF_SCOPE');
    return renderedCitationRange(body, content, selected.getRangeAt(0));
}
export function renderedCitationRange(body: HTMLElement, content: string, range: Range): LocatedCitation {
    if (!body.contains(range.startContainer) || !body.contains(range.endContainer))
        throw new StationError('不能跨消息或跨来源选择正文。', 'OUT_OF_SCOPE');
    const text = range.toString();
    if (!text.trim())
        throw new StationError('选区只有空白，请重新选择正文。', 'OUT_OF_SCOPE');
    const privateSelector = 'button,input,textarea,svg,[hidden],[aria-hidden="true"],[data-private]';
    const parent = (node: Node) => node.nodeType === 1 ? node as Element : node.parentElement;
    if (parent(range.startContainer)?.closest(privateSelector) || parent(range.endContainer)?.closest(privateSelector) || range.cloneContents().querySelector(privateSelector))
        throw new StationError('不能引用隐藏内容或操作控件。', 'OUT_OF_SCOPE');
    const positions: {
        char: string;
        start: number;
        end: number;
    }[] = [];
    function leaves(tokens: Token[], raw: string, offset: number) {
        let cursor = 0;
        for (const token of tokens) {
            const index = raw.indexOf(token.raw, cursor);
            if (index < 0)
                throw conflict();
            cursor = index + token.raw.length;
            const at = offset + index;
            if (token.type === 'html')
                throw conflict();
            if ('tokens' in token && token.tokens) {
                leaves(token.tokens, token.raw, at);
                continue;
            }
            if (token.type === 'list') {
                leaves(token.items as Token[], token.raw, at);
                continue;
            }
            if (['space', 'hr', 'br', 'def', 'image'].includes(token.type))
                continue;
            if (!['text', 'escape', 'codespan', 'code'].includes(token.type))
                throw conflict();
            const visible = token.type === 'codespan' || token.type === 'code' ? token.text : plain(token.raw);
            if (/&(?:#\d+|#x[\da-f]+|[a-z]+);/iu.test(token.raw))
                throw conflict();
            let scan = token.type === 'code' && /^ {0,3}(?:```|~~~)/u.test(token.raw) ? token.raw.indexOf('\n') + 1 : 0;
            for (const char of visible) {
                if (/\s/u.test(char))
                    continue;
                const found = token.raw.indexOf(char, scan);
                if (found < 0)
                    throw conflict();
                // Entity decoding is deliberately unsupported instead of assigning a wrong raw range.
                const end = found + char.length;
                positions.push({ char, start: at + found, end: at + end });
                scan = end;
            }
        }
    }
    leaves(marked.lexer(content), content, 0);
    if (positions.map(item => item.char).join('') !== compact(body.textContent || ''))
        throw conflict();
    const before = document.createRange();
    before.selectNodeContents(body);
    before.setEnd(range.startContainer, range.startOffset);
    const start = Array.from(compact(before.toString())).length, count = Array.from(compact(text)).length;
    if (!count || !positions[start] || !positions[start + count - 1])
        throw conflict();
    let left = positions[start].start, right = positions[start + count - 1].end;
    while (left > 0 && /[*_~`]/u.test(content[left - 1]))
        left--;
    while (right < content.length && /[*_~`]/u.test(content[right]))
        right++;
    const candidate = content.slice(left, right);
    if (compact(plain(candidate)) !== compact(text))
        throw conflict();
    return locateCitation(content, candidate, left);
}
