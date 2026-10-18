// Erreurs JavaScript remontées dans le journal de Kobi (KOB-167) : une
// fenêtre restée blanche dit pourquoi dans ~/Kobi/logs/kobi.log. Seulement
// le message et l'endroit, jamais de contenu de l'utilisateur.

interface Coque {
  core: { invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> };
}

let installee = false;

function signaler(erreur: string, source?: string, ligne?: number): void {
  const api = (window as unknown as { __TAURI__?: Coque }).__TAURI__;
  if (!api) return;
  void api.core
    .invoke('erreur_interface', { erreur: erreur.slice(0, 500), source: source ?? null, ligne: ligne ?? null })
    .catch(() => {});
}

export function remonterLesErreurs(): void {
  if (installee) return;
  installee = true;
  window.addEventListener('error', (e) => {
    signaler(e.message || String(e.error), e.filename, e.lineno);
  });
  window.addEventListener('unhandledrejection', (e) => {
    const r: unknown = e.reason;
    signaler(`promesse rejetée : ${r instanceof Error ? r.message : String(r)}`);
  });
}

remonterLesErreurs();
