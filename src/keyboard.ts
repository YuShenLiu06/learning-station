type KeyInput = { key: string; metaKey: boolean; ctrlKey: boolean; isComposing?: boolean; keyCode?: number };
export function isSubmitKey(event: KeyInput): boolean {
  return (event.metaKey || event.ctrlKey) && event.key === 'Enter' && !event.isComposing && event.keyCode !== 229;
}
