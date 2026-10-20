// Accès à la coque Tauri pour les fenêtres secondaires (Carnet, Réglages) :
// commandes et événements de `window.__TAURI__` (`withGlobalTauri`). Hors de
// Tauri (navigateur, tests sans fausse coque), `disponible()` est faux et
// toute commande est refusée avec un message lisible.

// Import circulaire avec i18n (qui lit la langue par `invoke`) : `t` n'est
// appelé qu'à l'exécution, une fois les deux modules chargés.
import { t } from './i18n';

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
  if (!api) return Promise.reject(new Error(t('kobi.ipc.horsDeKobi')));
  return api.core.invoke<T>(cmd, args);
}

/** Écoute un événement de la coque ; retourne de quoi arrêter d'écouter. */
export function ecouter<T>(event: string, handler: (payload: T) => void): () => void {
  const fin = coque()?.event.listen<T>(event, (e) => handler(e.payload));
  return () => void fin?.then((arreter) => arreter());
}

/**
 * La commande n'existe pas dans cette version de Kobi (fonction pas encore
 * livrée) : l'interface le dit au lieu d'afficher une erreur.
 */
export function absente(e: unknown): boolean {
  return /command \S+ not found|commande inconnue/i.test(message(e));
}
