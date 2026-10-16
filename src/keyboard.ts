type KeyInput = { key: string; shiftKey?: boolean; altKey?: boolean; repeat?: boolean; isComposing?: boolean; keyCode?: number };
export function composerKeyAction(event: KeyInput, composing = false): 'native' | 'send' | 'block' {
  if (event.key !== 'Enter' || composing || event.isComposing || event.keyCode === 229 || event.shiftKey || event.altKey) return 'native';
  return event.repeat ? 'block' : 'send';
}
