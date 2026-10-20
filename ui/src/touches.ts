// Touches d'un raccourci, en français : partagé par les réglages et le badge
// REC (KOB-250).

/** Touches d'un raccourci Tauri (`Ctrl+Shift+Space`), en français. */
export function touches(accelerateur: string): string[] {
  const noms: Record<string, string> = { space: 'Espace', control: 'Ctrl', ctrl: 'Ctrl', commandorcontrol: 'Ctrl', shift: 'Maj', alt: 'Alt', super: 'Win', enter: 'Entrée' };
  return accelerateur.split('+').map((t) => noms[t.trim().toLowerCase()] ?? t.trim().toUpperCase());
}
