// Skin choisi par l'utilisateur. Dans l'application, la coque Tauri le garde
// dans `~/Kobi/config.toml` (commandes `get_skin` / `set_skin`) et prévient
// toutes les fenêtres (événement `kobi://skin`). Dans un navigateur (tests,
// développement), le choix vit dans le stockage local et passe d'une page à
// l'autre par un BroadcastChannel.

import { DEFAULT_SKIN, SKINS } from './skins/registry';

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(name: string, cb: (e: { payload: T }) => void): Promise<() => void> };
}

const KEY = 'kobi.skin';
const EVENT = 'kobi://skin';

function tauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

function valid(id: unknown): id is string {
  return typeof id === 'string' && id in SKINS;
}

export async function loadSkin(): Promise<string> {
  const api = tauri();
  try {
    const id = api ? await api.core.invoke<string>('get_skin') : localStorage.getItem(KEY);
    return valid(id) ? id : DEFAULT_SKIN;
  } catch {
    return DEFAULT_SKIN;
  }
}

export async function saveSkin(id: string): Promise<void> {
  if (!valid(id)) throw new Error(`skin inconnu : ${id}`);
  const api = tauri();
  if (api) {
    await api.core.invoke('set_skin', { skin: id });
    return;
  }
  try {
    localStorage.setItem(KEY, id);
  } catch {
    // stockage indisponible : le choix vaut pour la page courante
  }
  new BroadcastChannel(EVENT).postMessage(id);
}

/** Appelle `cb` à chaque changement de skin fait ailleurs (fenêtre des réglages). */
export function onSkinChange(cb: (id: string) => void): void {
  const api = tauri();
  if (api) {
    void api.event.listen<string>(EVENT, (e) => valid(e.payload) && cb(e.payload));
    return;
  }
  new BroadcastChannel(EVENT).onmessage = (e: MessageEvent<unknown>) => {
    if (valid(e.data)) cb(e.data);
  };
}
