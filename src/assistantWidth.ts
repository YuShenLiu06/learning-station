export const defaultAssistantWidth = 346;
export function validAssistantWidth(value: number) { return Number.isInteger(value) && value >= 280 && value <= 960; }
export function clampAssistantWidth(value: number, maximum = 960) { return Math.round(Math.max(280, Math.min(960, maximum, value))); }
