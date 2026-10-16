export const defaultSidebarWidth = 240;
export function validSidebarWidth(value: number) { return Number.isInteger(value) && value >= 180 && value <= 480; }
export function clampSidebarWidth(value: number, maximum = 480) { return Math.round(Math.max(180, Math.min(value, 480, Math.max(180, maximum)))); }
