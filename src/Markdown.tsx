import { memo, useMemo } from 'react';
import { sanitizeMarkdown } from './sanitize';

// Untrusted material is display-only; remote media never loads automatically.
export const Markdown = memo(function Markdown({ text, className = '' }: { text: string; className?: string }) {
  const html = useMemo(() => sanitizeMarkdown(text), [text]);
  return <div className={`markdown ${className}`} dangerouslySetInnerHTML={{ __html: html }} />;
});
