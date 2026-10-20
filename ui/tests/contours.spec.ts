// Aucun contour coupé (KOB-246) : les contours vert acide (bordure, anneau
// en ombre portée, outline, contour du texte) restent entiers, ni hors de la
// fenêtre transparente ni rognés par un conteneur qui coupe ce qui dépasse,
// dans chaque état et aux échelles 100 % et 150 %. La vue agrandie est posée
// comme par la coque : au ras de la fenêtre agrandie.
import { expect, test, type Page } from '@playwright/test';
import { emit, mockTauri, openUi } from './helpers';

const ACIDE = 'rgb(215, 255, 58)';

/** Éléments visibles dont le contour acide (débord compris) sort de la fenêtre ou d'un parent qui rogne. */
async function rognes(page: Page): Promise<string[]> {
  return page.evaluate((ACIDE) => {
    const px = (v: string) => parseFloat(v) || 0;
    const nom = (e: Element) => `${e.tagName.toLowerCase()}${e.id ? '#' + e.id : ''}${e.className && typeof e.className === 'string' ? '.' + e.className.trim().split(/\s+/).slice(0, 3).join('.') : ''}`;
    const sortie: string[] = [];
    for (const el of Array.from(document.querySelectorAll<HTMLElement>('body *'))) {
      const cs = getComputedStyle(el);
      if (cs.display === 'none' || cs.visibility === 'hidden' || px(cs.opacity) === 0) continue;
      // Débord hors de la boîte de bordure, côté par côté : contour acide
      // (outline, contour du texte) et toutes les ombres d'un élément cerné
      // d'acide (anneau et ombre portée, chacune avec son décalage).
      const ombres = [...cs.boxShadow.matchAll(/(rgba?\([^)]*\))\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+([\d.]+)px\s+(-?[\d.]+)px(\s+inset)?/g)];
      const trait = (cs as unknown as Record<string, string>).webkitTextStrokeColor ?? '';
      const contourAcide = cs.outlineStyle !== 'none' && cs.outlineColor === ACIDE;
      const bordure = [cs.borderTopColor, cs.borderRightColor, cs.borderBottomColor, cs.borderLeftColor].includes(ACIDE) && px(cs.borderTopWidth) > 0;
      const anneau = ombres.some((o) => o[1] === ACIDE && !o[6]);
      const texte = trait === ACIDE;
      if (!contourAcide && !bordure && !anneau && !texte) continue;
      const deb = { l: 0, t: 0, r: 0, b: 0 };
      const etendre = (l: number, t: number, r: number, b: number) => {
        deb.l = Math.max(deb.l, l);
        deb.t = Math.max(deb.t, t);
        deb.r = Math.max(deb.r, r);
        deb.b = Math.max(deb.b, b);
      };
      for (const o of ombres) {
        if (o[6]) continue;
        const [x, y, flou, etendue] = [px(o[2]!), px(o[3]!), px(o[4]!), px(o[5]!)];
        const d = etendue + flou;
        etendre(d - x, d - y, d + x, d + y);
      }
      if (contourAcide) {
        const d = px(cs.outlineWidth) + px(cs.outlineOffset);
        etendre(d, d, d, d);
      }
      // Contour du texte seul : l'étendue des glyphes, pas la boîte (le grand
      // mot est centré dans une boîte plus large que lui).
      let r = el.getBoundingClientRect();
      if (texte) {
        const d = px((cs as unknown as Record<string, string>).webkitTextStrokeWidth ?? '') / 2;
        etendre(d, d, d, d);
        if (!bordure && !anneau && !contourAcide) {
          const plage = document.createRange();
          plage.selectNodeContents(el);
          r = plage.getBoundingClientRect();
        }
      }
      if (r.width === 0 || r.height === 0) continue;
      const b = { l: r.left - deb.l, t: r.top - deb.t, r: r.right + deb.r, b: r.bottom + deb.b };
      const pb: string[] = [];
      if (b.l < -0.5 || b.t < -0.5 || b.r > innerWidth + 0.5 || b.b > innerHeight + 0.5) pb.push(`fenêtre (${Math.round(b.l)},${Math.round(b.t)},${Math.round(b.r)},${Math.round(b.b)} / ${innerWidth}×${innerHeight})`);
      for (let p = el.parentElement; p && p !== document.body; p = p.parentElement) {
        const ps = getComputedStyle(p);
        if (ps.overflow === 'visible' && ps.overflowX === 'visible' && ps.overflowY === 'visible' && ps.clipPath === 'none') continue;
        // Une zone qui défile cache ce qui dépasse par construction : ce qui
        // en sort se lit en défilant, ce n'est pas un contour coupé.
        if (/auto|scroll/.test(`${ps.overflowX} ${ps.overflowY}`)) continue;
        const q = p.getBoundingClientRect();
        const marge = (b.l < q.left - 0.5 ? 'g' : '') + (b.t < q.top - 0.5 ? 'h' : '') + (b.r > q.right + 0.5 ? 'd' : '') + (b.b > q.bottom + 0.5 ? 'b' : '');
        if (marge) pb.push(`rogné par ${nom(p)} [${marge}] (overflow ${ps.overflow}${ps.clipPath !== 'none' ? ', clip-path' : ''}, rayon ${ps.borderRadius})`);
      }
      if (pb.length) sortie.push(`${nom(el)} débord ${JSON.stringify(deb)} : ${pb.join(' ; ')}`);
    }
    return sortie;
  }, ACIDE);
}

const md = '**Deux pistes :**\n\n- `api/src/auth/refresh.ts`, ligne 88\n- `app/lib/session.dart`, ligne 41\n\n' + 'La durée actuelle est de quinze minutes. '.repeat(12);
/**
 * Disposition de la coque (agrandir.rs) : fenêtre agrandie au ras du panneau,
 * posé au coin haut-gauche, et la scène de Kobi entière à sa droite.
 */
const DISPOSITION = { panneau: { x: 0, y: 0, largeur: 480, hauteur: 560 }, kobi: { x: 480, y: 0, largeur: 820, hauteur: 560 } };
const REGLAGE = { id: '4', type: 'validation', genre: 'reglage', resume: 'Changer le réglage « voice.meeting » : false → true', detail: { cle: 'voice.meeting', avant: false, apres: true }, echeance_ms: 60000 };

for (const echelle of [1, 1.5]) {
  test.describe(`aucun contour coupé, échelle ${echelle * 100} %`, () => {
    test.use({ deviceScaleFactor: echelle });
    const etats: Array<[string, (p: Page) => Promise<void>]> = [
      ['repos', async () => {}],
      ['bulle', async (p) => {
        await emit(p, 'state', { etat: 'veille', activite: 'repond', mascotte: 'answer', transcrit: false });
        await emit(p, 'answer', md);
        await p.waitForTimeout(2500);
      }],
      ['carte', async (p) => {
        await p.evaluate((c) => window.kobi.carte(c as never), REGLAGE);
        await p.waitForTimeout(800);
      }],
      ['vue', async (p) => {
        // La coque agrandit la fenêtre au ras du panneau (agrandir.rs).
        await p.setViewportSize({ width: 480 + 820, height: 560 });
        await emit(p, 'conversation', [{ tour: 1, question: 'où est le refresh ?', reponse: md, statut: 'fini', clavier: true, etapes: [] }]);
        await emit(p, 'vue', { action: 'ouvrir' });
        await p.waitForTimeout(1200);
      }],
      ['menu', async (p) => {
        const box = (await p.locator('.kobi').boundingBox())!;
        await p.mouse.click(box.x + box.width / 2, box.y + box.height / 2, { button: 'right' });
        await p.waitForTimeout(600);
      }],
      ['rec', async (p) => {
        await emit(p, 'enregistrement', { actif: true, pause: false, debut_ms: Date.now() - 65_000, notes: 2, titre: null });
        await emit(p, 'state', { etat: 'reunion', activite: null, mascotte: 'idle', transcrit: true });
        await p.waitForTimeout(800);
      }],
      ['ecoute', async (p) => {
        await emit(p, 'state', { etat: 'assistant', activite: 'ecoute', mascotte: 'listen', transcrit: false });
        await emit(p, 'question', 'Kobi, où est le refresh du jeton ?');
        await p.waitForTimeout(800);
      }],
    ];
    for (const [nom, poser] of etats) {
      test(nom, async ({ page }) => {
        await mockTauri(page, { agrandir_kobi: DISPOSITION });
        await openUi(page, '');
        await poser(page);
        expect(await rognes(page)).toEqual([]);
      });
    }
  });
}
