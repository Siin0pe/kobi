// Accès à la coque Tauri pour les fenêtres secondaires (Carnet, Réglages) :
// commandes et événements de `window.__TAURI__` (`withGlobalTauri`). Hors de
// Tauri (navigateur, tests sans fausse coque), `disponible()` est faux et
// toute commande est refusée avec un message lisible.

interface Coque {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, handler: (e: { payload: T }) => void): Promise<() => void> };
}

function coque(): Coque | undefined {
  return (window as unknown as { __TAURI__?: Coque }).__TAURI__;
}

export function disponible(): boolean {
  return coque() !== undefined;
}

/** Message d'un refus de commande (chaîne envoyée par Rust, ou erreur JS). */
export function message(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Commande de `kobi-app` ; rejette avec le message d'erreur de Rust. */
export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const api = coque();
  if (!api) return Promise.reject(new Error('Ouvrez cette fenêtre depuis Kobi.'));
  return api.core.invoke<T>(cmd, args);
}

/** Écoute un événement de la coque ; sans coque, ne fait rien. */
export function ecouter<T>(event: string, handler: (payload: T) => void): void {
  void coque()?.event.listen<T>(event, (e) => handler(e.payload));
}
