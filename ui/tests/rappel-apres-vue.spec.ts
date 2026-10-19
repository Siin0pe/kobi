// KOB-176 : la vue agrandie de la conversation (KOB-169) cache les
// notifications pendant la lecture ; un rappel qui arrive alors la range et
// se montre, quel que soit le réglage d'ouverture de la vue.
import { expect, test, type Page } from '@playwright/test';
import { emit, expectNoErrors, mockTauri, openUi } from './helpers';

const vue = (page: Page) => page.getByRole('dialog', { name: 'Conversation avec Kobi' });

async function kobi(page: Page, reglages: Record<string, unknown>): Promise<string[]> {
  await page.setViewportSize({ width: 410, height: 280 });
  await page.exposeFunction('__resize', (w: number, h: number) => page.setViewportSize({ width: w, height: h }));
  await mockTauri(page);
  await page.addInitScript((r) => localStorage.setItem('kobi.affichage', JSON.stringify(r)), reglages);
  await page.addInitScript(() => {
    const w = window as unknown as { __TAURI__: { core: { invoke: (c: string, a?: Record<string, number | string | boolean>) => Promise<unknown> } }; __resize(w: number, h: number): Promise<void> };
    const base = w.__TAURI__.core.invoke;
    w.__TAURI__.core.invoke = async (cmd, args) => {
      const out = await base(cmd, args);
      if (cmd !== 'agrandir_kobi') return out;
      if (!args?.grand) {
        if (args?.appliquer) await w.__resize(410, 280);
        return null;
      }
      const lp = Number(args.largeur), hp = Number(args.hauteur);
      const W = lp + 230, H = Math.max(hp + 32, 280);
      if (args.appliquer) await w.__resize(W, H);
      return { kobi: { x: W - 410, y: H - 280, largeur: 410, hauteur: 280 }, panneau: { x: 16, y: H - 16 - hp, largeur: lp, hauteur: hp } };
    };
  });
  return openUi(page, '');
}

const etat = (mascotte: string) => ({ etat: 'assistant', activite: mascotte === 'answer' ? 'repond' : null, mascotte, transcrit: false });
const LONGUE = Array.from({ length: 12 }, (_, i) => `Ligne ${i + 1} : reçu, je regarde le nom du projet.`).join('\n\n');
const rappel = (id: string) => ({ id, texte: 'sortir le linge', echeance_ms: Date.now() });

async function reponseLongue(page: Page): Promise<void> {
  await emit(page, 'state', etat('listen'));
  await emit(page, 'question', 'tu peux me rappeler le nom du projet');
  await emit(page, 'state', etat('answer'));
  await emit(page, 'answer', LONGUE);
  await emit(page, 'conversation', [{ tour: 1, question: 'tu peux me rappeler le nom du projet', reponse: LONGUE, statut: 'fini', clavier: false }]);
  await page.clock.runFor(3_000);
  await expect(vue(page)).toBeVisible();
}

test('la souris retient la vue agrandie : le rappel qui arrive la range et se montre', async ({ page }) => {
  await page.clock.install();
  const errors = await kobi(page, { auto: 'longues' });
  await reponseLongue(page);
  await emit(page, 'state', etat('idle'));
  // Le pointeur reste sur le panneau (comme sous Xvfb) : la vue ne se range pas seule.
  const v = (await vue(page).boundingBox())!;
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  await page.clock.runFor(60_000);
  await expect(vue(page)).toBeVisible();
  await emit(page, 'rappel', rappel('r1'));
  await page.clock.runFor(1_000);
  await expect(vue(page)).toBeHidden();
  await expect(page.locator('.stage')).not.toHaveClass(/vue-ouverte/);
  const notif = page.locator('.notif-reminder');
  await expect(notif).toBeVisible();
  await expect(notif.locator('.notif-text')).toHaveText('sortir le linge');
  // Le rappel ne s'ajoute pas à la conversation et ne rouvre pas la vue.
  await page.clock.runFor(5_000);
  await expect(vue(page)).toBeHidden();
  await expect(notif).toBeVisible();
  await expectNoErrors(errors);
});

test('vue réglée sur « toujours » : le rappel ne la rouvre pas et reste visible', async ({ page }) => {
  await page.clock.install();
  const errors = await kobi(page, { auto: 'toujours', delai: 10 });
  await reponseLongue(page);
  await emit(page, 'state', etat('idle'));
  await page.clock.runFor(12_000);
  await expect(vue(page)).toBeHidden();
  await emit(page, 'rappel', rappel('r2'));
  await page.clock.runFor(3_000);
  await expect(vue(page)).toBeHidden();
  await expect(page.locator('.notif-reminder')).toBeVisible();
  await expectNoErrors(errors);
});

test('comme l’essai réel : deuxième échange pendant la vue, Kobi s’endort, puis le rappel apparaît', async ({ page }) => {
  await page.clock.install();
  const errors = await kobi(page, { auto: 'longues' });
  await reponseLongue(page);
  await emit(page, 'state', etat('idle'));
  await page.clock.runFor(10_000);
  const b2 = { tour: 2, question: 'rappelle-moi dans une minute de sortir le linge', reponse: 'C’est noté : je te rappelle dans 1 minute de sortir le linge.', statut: 'fini', clavier: false };
  await emit(page, 'state', etat('listen'));
  await emit(page, 'question', b2.question);
  await emit(page, 'state', etat('answer'));
  await emit(page, 'answer', b2.reponse);
  await page.clock.runFor(3_000);
  await emit(page, 'state', etat('idle'));
  await page.clock.runFor(60_000);
  await emit(page, 'rappel', rappel('r3'));
  await page.clock.runFor(1_000);
  await expect(page.locator('.notif-reminder')).toBeVisible();
  await expectNoErrors(errors);
});
