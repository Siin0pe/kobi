// Petite fabrique d'éléments pour les fenêtres secondaires. Le texte passe
// toujours par des nœuds texte : rien de ce qui vient du disque n'est
// interprété comme du HTML.

type Enfant = Node | string | null | undefined | false;
type Attributs = Record<string, string | number | boolean | null | undefined | EventListener>;

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attributs = {},
  ...enfants: Enfant[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [cle, valeur] of Object.entries(attrs)) {
    if (valeur === null || valeur === undefined || valeur === false) continue;
    if (cle.startsWith('on') && typeof valeur === 'function') el.addEventListener(cle.slice(2), valeur);
    else if (cle === 'class') el.className = String(valeur);
    else el.setAttribute(cle, valeur === true ? '' : String(valeur));
  }
  for (const e of enfants) if (e !== null && e !== undefined && e !== false) el.append(e);
  return el;
}

/** Texte avec les occurrences de `mot` (sans casse ni accents) dans des `<mark>`. */
export function surligner(texte: string, mot: string): Node[] {
  const aiguille = normaliser(mot.trim());
  if (!aiguille) return [document.createTextNode(texte)];
  // La normalisation garde une lettre par lettre (on retire seulement les
  // diacritiques combinants) : les positions se correspondent.
  const plat = Array.from(texte).map((c) => normaliser(c) || c).join('');
  const out: Node[] = [];
  let i = 0;
  const chars = Array.from(texte);
  const platChars = Array.from(plat);
  const cible = Array.from(aiguille);
  while (i < chars.length) {
    const j = indexDe(platChars, cible, i);
    if (j < 0) break;
    if (j > i) out.push(document.createTextNode(chars.slice(i, j).join('')));
    out.push(h('mark', {}, chars.slice(j, j + cible.length).join('')));
    i = j + cible.length;
  }
  if (i < chars.length) out.push(document.createTextNode(chars.slice(i).join('')));
  return out;
}

function indexDe(foin: string[], aiguille: string[], depuis: number): number {
  outer: for (let i = depuis; i + aiguille.length <= foin.length; i++) {
    for (let k = 0; k < aiguille.length; k++) if (foin[i + k] !== aiguille[k]) continue outer;
    return i;
  }
  return -1;
}

/** Minuscules sans accents, comme la recherche de `kobi-session`. */
export function normaliser(texte: string): string {
  return texte.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
}

/**
 * Les animations sont-elles réduites ? Seul le réglage « Animations » de
 * Kobi en décide (KOB-149, défaut « toujours ») : il pose la classe
 * `reduced-motion` sur l'élément racine. Le média `prefers-reduced-motion`
 * n'est jamais lu directement (les effets de Windows désactivés figeaient
 * Kobi).
 */
export function animationsReduites(): boolean {
  return document.documentElement.classList.contains('reduced-motion');
}
