// Intégration à la coque Tauri (API globale `window.__TAURI__`, activée par
// `withGlobalTauri`). Hors de Tauri (navigateur, tests), rien ne se passe.

interface TauriGlobal {
  window: { getCurrentWindow(): { startDragging(): Promise<void> } };
}

function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

export function isNative(): boolean {
  return tauri() !== undefined;
}

/** Glisser la mascotte déplace la fenêtre ; les cartes et leurs boutons restent cliquables. */
export function enableWindowDrag(root: HTMLElement): void {
  const api = tauri();
  if (!api) return;
  root.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    const target = e.target as Element | null;
    if (target?.closest('button, a, input, textarea, select, .card')) return;
    e.preventDefault();
    void api.window.getCurrentWindow().startDragging();
  });
}
