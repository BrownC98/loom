export const UI_ZOOM_VAR = '--ui-zoom';

export const uiZoom = (): number => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(UI_ZOOM_VAR)) || 1;
