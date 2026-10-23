import { marked, type Token } from 'marked';
import DOMPurify from 'dompurify';

export function sanitizeMarkdown(text: string): string {
  return sanitizeHtml(marked.parse(text, { async: false, breaks: false }) as string);
}
export function sanitizeMarkdownTokens(tokens: Token[]): string { return sanitizeHtml(marked.parser(tokens)); }
function sanitizeHtml(html: string): string {
  return DOMPurify.sanitize(html, {
      ALLOWED_TAGS: ['p', 'br', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'strong', 'em', 'del', 's', 'ul', 'ol', 'li', 'pre', 'code', 'blockquote', 'hr', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'sup', 'sub', 'kbd'],
      ALLOWED_ATTR: ['title', 'start', 'colspan', 'rowspan'],
      ALLOW_DATA_ATTR: false,
      ALLOW_ARIA_ATTR: false,
      ALLOWED_URI_REGEXP: /^(?:https?:\/\/|mailto:|#)/i,
    });
}
