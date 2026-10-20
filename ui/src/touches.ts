// Touches d'un raccourci, dans la langue de l'interface : partagé par les réglages et le badge
// REC (KOB-250).

import { t } from './i18n';

/** Touches d'un raccourci Tauri (`Ctrl+Shift+Space`), dans la langue de l'interface. */
export function touches(accelerateur: string): string[] {
  const noms: Record<string, string> = { space: t('touches.espace'), control: 'Ctrl', ctrl: 'Ctrl', commandorcontrol: 'Ctrl', shift: t('touches.maj'), alt: 'Alt', super: 'Win', enter: t('touches.entree') };
  return accelerateur.split('+').map((k) => noms[k.trim().toLowerCase()] ?? k.trim().toUpperCase());
}
