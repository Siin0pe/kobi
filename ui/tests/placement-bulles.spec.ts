// Placement des bulles (KOB-216) : une bulle (réponse, suggestion, erreur,
// rédaction, carte de validation) ne mord jamais sur la tête de Kobi, pointe
// et ombre comprises ; sa pointe part vers sa bouche. Vérifié dans le temps
// (la carte flotte, Kobi bouge) et pendant une réponse longue qui s'écrit.
import { expect, test, type Page } from '@playwright/test';
import { expectNoErrors, openUi } from './helpers';

/** Pointe (à droite) et ombre portée : elles comptent dans la bulle. */
const POINTE = 18;
const OMBRE = 8;

interface Mesure {
  /** Plus grande surface commune entre une bulle (élargie) et la tête, en px². */
  chevauchement: number;
  quoi: string;
  /** Écart vertical entre la pointe et la bouche, pour chaque bulle qui couvre sa hauteur. */
  pointes: number[];
}

const mesurer = (page: Page): Promise<Mesure> =>
  page.evaluate(
    ([pointe, ombre]) => {
      const tete = document.querySelector('.kobi .pill')!.getBoundingClientRect();
      const bouche = document.querySelector('.kobi .mouth')!.getBoundingClientRect();
      const yBouche = bouche.top + bouche.height / 2;
      // La tête est une gélule : demi-cercles à gauche et à droite.
      const r = tete.height / 2;
      const dansLaTete = (x: number, y: number): boolean => {
        if (y < tete.top || y > tete.bottom || x < tete.left || x > tete.right) return false;
        const cx = Math.min(Math.max(x, tete.left + r), tete.right - r);
        return (x - cx) ** 2 + (y - (tete.top + r)) ** 2 <= r * r;
      };
      let chevauchement = 0;
      let quoi = '';
      const pointes: number[] = [];
      for (const c of document.querySelectorAll<HTMLElement>('.bubbles .card, .kval')) {
        if (c.offsetParent === null) continue;
        const b = c.getBoundingClientRect();
        // Pointe et ombre en px de scène : à l'échelle de la fenêtre.
        const s = b.width / c.offsetWidth;
        // Corps de la bulle, ombre comprise, contre la boîte de la tête.
        const x = Math.max(0, Math.min(b.right + ombre * s, tete.right) - Math.max(b.left, tete.left));
        const y = Math.max(0, Math.min(b.bottom + ombre * s, tete.bottom) - Math.max(b.top, tete.top));
        const r2 = { x, y };
        let aire = r2.x * r2.y;
        let detail = `corps ${Math.round(x)}×${Math.round(y)}`;
        // Bout de la pointe, contre la forme de la tête.
        const p = getComputedStyle(c, '::before');
        if (p.content !== 'none') {
          const yPointe = b.top + parseFloat(p.top) * s;
          if (dansLaTete(b.right + pointe * s, yPointe)) {
            aire = Math.max(aire, 1);
            detail += ' + pointe dans la tête';
          }
          if (yBouche > b.top && yBouche < b.bottom) pointes.push(Math.abs(yPointe - yBouche));
        }
        if (aire > chevauchement) {
          chevauchement = aire;
          quoi = `${c.className} ${detail} (bulle ${Math.round(b.left)}-${Math.round(b.right)}×${Math.round(b.top)}-${Math.round(b.bottom)}, tête ${Math.round(tete.left)}-${Math.round(tete.right)}×${Math.round(tete.top)}-${Math.round(tete.bottom)})`;
        }
      }
      return { chevauchement, quoi, pointes };
    },
    [POINTE, OMBRE] as const,
  );

/** Relève la pire mesure sur `n` relevés espacés (flottement, gestes). */
async function pire(page: Page, n = 8, pas = 350): Promise<Mesure> {
  let m: Mesure = { chevauchement: 0, quoi: '', pointes: [] };
  for (let i = 0; i < n; i++) {
    const r = await mesurer(page);
    if (r.chevauchement >= m.chevauchement) m = { ...r, pointes: [...m.pointes, ...r.pointes] };
    else m.pointes.push(...r.pointes);
    await page.waitForTimeout(pas);
  }
  return m;
}

for (const [largeur, hauteur] of [
  [820, 560],
  [410, 280],
  [1180, 760],
] as const) {
  test(`aucune bulle ne mord sur la tête (${largeur}×${hauteur})`, async ({ page }) => {
    await page.setViewportSize({ width: largeur, height: hauteur });
    const errors = await openUi(page, '');
    for (const etat of ['answer', 'suggest', 'error', 'compose'] as const) {
      await page.evaluate((s) => window.kobi.setState(s, window.kobi.demoData(s)), etat);
      await expect(page.locator('.bubbles .card').first()).toBeVisible();
      // Après l'entrée (la carte sort de sa bouche, testé à part).
      await page.waitForTimeout(700);
      const m = await pire(page);
      expect(m.chevauchement, `${etat} : ${m.quoi}`).toBe(0);
    }
    await expectNoErrors(errors);
  });
}

test('à son entrée, la bulle sort de sa bouche : elle frôle le bord de la tête, jamais le visage', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await expect(page.locator('.bubbles .card-answer')).toBeVisible();
  // Relevés serrés pendant l'entrée (560 ms).
  let plusLoin = 0;
  for (let i = 0; i < 12; i++) {
    plusLoin = Math.max(
      plusLoin,
      await page.evaluate(() => {
        const tete = document.querySelector('.kobi .pill')!.getBoundingClientRect();
        const b = document.querySelector('.bubbles .card-answer')!.getBoundingClientRect();
        return b.bottom > tete.top && b.top < tete.bottom ? Math.max(0, b.right - tete.left) : 0;
      }),
    );
    await page.waitForTimeout(50);
  }
  // Au plus le bord de la tête, à hauteur de la bouche (chevauchement voulu, comme une parole).
  expect(plusLoin).toBeLessThanOrEqual(48);
  await expectNoErrors(errors);
});

test('la pointe part vers la bouche de Kobi', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await expect(page.locator('.bubbles .card-answer')).toBeVisible();
  // Réponse entièrement écrite : la bulle a sa hauteur finale.
  const texte = await page.evaluate(() => String(window.kobi.demoData('answer').answer));
  await expect(page.locator('.bubbles .card-answer .defile')).toContainText(texte.slice(-20), { timeout: 15_000 });
  await page.waitForTimeout(300);
  const m = await mesurer(page);
  expect(m.pointes.length).toBeGreaterThan(0);
  // À quelques pixels près : la carte flotte et penche un peu, et la pointe
  // garde ses distances avec les coins arrondis.
  for (const d of m.pointes) expect(d).toBeLessThan(24);
  await expectNoErrors(errors);
});

test('réponse longue en flux : la bulle grandit sans jamais mordre sur Kobi', async ({ page }) => {
  const errors = await openUi(page, '');
  const longue = Array.from({ length: 14 }, (_, i) => `Phrase ${i + 1} de la réponse, avec quelques mots de plus pour remplir la bulle.`).join(' ');
  await page.evaluate((a) => window.kobi.setState('answer', { question: 'Explique tout ?', answer: a }), longue);
  const carte = page.locator('.bubbles .card-answer');
  await expect(carte).toBeVisible();
  await page.waitForTimeout(700);
  // Relevés pendant que le texte s'écrit (la bulle grandit jusqu'à sa hauteur maximale).
  const m = await pire(page, 14, 300);
  expect(m.chevauchement, m.quoi).toBe(0);
  await expectNoErrors(errors);
});

test('carte de validation : à côté de Kobi, pas sur lui', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() =>
    window.kobi.carte({ id: '1', type: 'validation', genre: 'commande', resume: 'Pousser la branche', detail: { commande: 'git push', dossier: '/code/kobi' }, echeance_ms: 60000 } as never),
  );
  await expect(page.locator('.kval')).toBeVisible();
  await page.waitForTimeout(700);
  const m = await pire(page, 6);
  expect(m.chevauchement, m.quoi).toBe(0);
  await expectNoErrors(errors);
});

test('fenêtre redimensionnée en cours de réponse : la bulle reste à l’écart', async ({ page }) => {
  const errors = await openUi(page, '');
  await page.evaluate(() => window.kobi.setState('answer', window.kobi.demoData('answer')));
  await expect(page.locator('.bubbles .card-answer')).toBeVisible();
  await page.waitForTimeout(700);
  for (const [w, h] of [
    [600, 410],
    [1000, 680],
    [820, 560],
  ] as const) {
    await page.setViewportSize({ width: w, height: h });
    const m = await pire(page, 3, 250);
    expect(m.chevauchement, `${w}×${h} : ${m.quoi}`).toBe(0);
  }
  await expectNoErrors(errors);
});
