// Animations de l'app Kobi (KOB-151) : transitions entre sections, survols
// qui ne déplacent jamais un bouton, petit Kobi qui vit et réagit, titres
// vivants, pause fenêtre masquée, animations réduites. Fausse coque Tauri.
import { expect, test, type Locator, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';
import { expectNoErrors } from './helpers';

async function ouvrirApp(page: Page, ancre = '#notes'): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.addInitScript(fakeTauri);
  await page.setViewportSize({ width: 1180, height: 760 });
  await page.goto(`/app.html${ancre}`);
  await page.evaluate(() => document.fonts.ready);
  return errors;
}

/** La boîte d'un bouton n'a pas bougé d'un pixel après le survol. */
async function immobileAuSurvol(page: Page, bouton: Locator): Promise<void> {
  await page.mouse.move(2, 2);
  await page.waitForTimeout(400);
  const avant = await bouton.boundingBox();
  await bouton.hover();
  // Fin des transitions de survol (0,3 s au plus).
  await page.waitForTimeout(450);
  const apres = await bouton.boundingBox();
  expect(apres, await bouton.evaluate((b) => b.outerHTML.slice(0, 80))).toEqual(avant);
}

test('les boutons ne bougent jamais sous le curseur', async ({ page }) => {
  const errors = await ouvrirApp(page);
  await immobileAuSurvol(page, page.locator('.nav-entree[data-section="rappels"]'));
  await immobileAuSurvol(page, page.locator('.vue-notes .element').first());
  await immobileAuSurvol(page, page.locator('.vue-notes .filtre').first());
  await immobileAuSurvol(page, page.locator('.bandeau-rec'));
  await page.getByRole('button', { name: 'Réglages', exact: true }).click();
  await immobileAuSurvol(page, page.locator('.vue-reglages .menu-entree').first());
  await immobileAuSurvol(page, page.locator('.vue-reglages .bouton').first());
  await immobileAuSurvol(page, page.locator('.vue-reglages .segment').first());
  await expectNoErrors(errors);
});

test('changement de section : l’ancienne s’efface pendant que la nouvelle arrive', async ({ page }) => {
  const errors = await ouvrirApp(page);
  await page.getByRole('button', { name: 'Rappels et tâches' }).click();
  // Plus bas dans la navigation : arrive d'en bas ; les notes sortent.
  await expect(page.locator('.vue-rappels')).toHaveAttribute('data-sens', 'bas');
  await expect(page.locator('.vue-notes')).toHaveClass(/sort/);
  await expect(page.locator('.vue-notes')).toHaveAttribute('hidden', '');
  await expect(page.locator('.vue-notes')).not.toHaveClass(/sort/);
  await page.getByRole('button', { name: 'Notes et réunions' }).click();
  await expect(page.locator('.vue-notes')).toHaveAttribute('data-sens', 'haut');
  // Une seule section accessible à la fois, même pendant la sortie.
  await expect(page.locator('.vue:not([hidden])')).toHaveCount(1);
  await expectNoErrors(errors);
});

test('le petit Kobi réagit à ce qu’on fait, puis revient au mot de la section', async ({ page }) => {
  const errors = await ouvrirApp(page);
  const kobi = page.locator('.nav-kobi');
  await expect(kobi.locator('.mot')).toHaveText('notes');
  await page.locator('.vue-notes .recherche input').fill('budget');
  await expect(kobi).toHaveAttribute('data-reaction', 'cherche');
  await expect(kobi.locator('.mot')).toHaveText('hmm…');
  await expect(kobi).not.toHaveAttribute('data-reaction', /./, { timeout: 4000 });
  await expect(kobi.locator('.mot')).toHaveText('notes');
  // Un rappel fait : « fait ! ».
  await page.getByRole('button', { name: 'Rappels et tâches' }).click();
  await page.locator('.vue-rappels .coche').first().click();
  await expect(kobi).toHaveAttribute('data-reaction', 'fait');
  await expect(kobi.locator('.mot')).toHaveText('fait !');
  // Kobi occupé (il réfléchit) : la mascotte a la priorité, pas de réaction.
  await page.evaluate(() =>
    (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('state', { etat: 'assistant', activite: null, mascotte: 'think', transcrit: false }),
  );
  await expect(kobi.locator('.mot')).toHaveText('hmm…');
  await expect(kobi).not.toHaveAttribute('data-reaction', /./);
  await expectNoErrors(errors);
});

test('titres vivants et petit Kobi qui vit au repos', async ({ page }) => {
  await page.clock.install();
  const errors = await ouvrirApp(page, '#rappels');
  const titre = page.locator('.vue-rappels .titre');
  await page.clock.runFor(2_000);
  await expect(titre).toHaveText('À ne pas oublier');
  // Une lettre se brouille au bout de 7 à 13 s, puis revient.
  const vus = new Set<string>();
  for (let i = 0; i < 160; i++) {
    await page.clock.runFor(100);
    vus.add((await titre.textContent()) ?? '');
  }
  expect([...vus].some((t) => t !== 'À ne pas oublier' && t.length === 'À ne pas oublier'.length)).toBe(true);
  await page.clock.runFor(2_000);
  await expect(titre).toHaveText('À ne pas oublier');
  await expectNoErrors(errors);
});

test('fenêtre masquée : les animations s’arrêtent ; animations réduites : rien ne bouge', async ({ page }) => {
  await page.addInitScript(() => {
    const poser = () => document.documentElement?.classList.add('reduced-motion');
    poser();
    document.addEventListener('readystatechange', poser);
  });
  const errors = await ouvrirApp(page);
  await page.getByRole('button', { name: 'Rappels et tâches' }).click();
  // Pas de sortie animée, rien qui tourne, même le REC.
  await expect(page.locator('.vue.sort')).toHaveCount(0);
  expect(await page.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length)).toBe(0);
  await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(page.locator('html')).toHaveClass(/app-masquee/);
  await expectNoErrors(errors);
});

test('fenêtre masquée sans réglage : toutes les animations CSS en pause', async ({ page }) => {
  const errors = await ouvrirApp(page);
  await page.evaluate(() => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await expect(page.locator('html')).toHaveClass(/app-masquee/);
  // Une lettre posée par le brouilleur finit son petit saut (0,36 s), puis plus rien.
  await page.waitForTimeout(1500);
  const enCours = await page.evaluate(() =>
    document
      .getAnimations()
      .filter((a) => a.playState === 'running')
      .map((a) => `${(a as CSSAnimation).animationName ?? 'js'} sur ${((a.effect as KeyframeEffect).target as Element)?.className}`),
  );
  expect(enCours).toEqual([]);
  await expectNoErrors(errors);
});

test('compteur REC aux chiffres qui roulent', async ({ page }) => {
  const errors = await ouvrirApp(page);
  const duree = page.locator('.bandeau-rec .rec-duree');
  await expect(duree).toHaveText(/^0:5\d$|^1:0\d$/);
  await expect(duree.locator('.chiffre.roule')).not.toHaveCount(0, { timeout: 3000 });
  await expectNoErrors(errors);
});

test('au repos, aucune animation sans fin à 60 images/s (budget CPU)', async ({ page }) => {
  const errors = await ouvrirApp(page);
  // Hors réunion : pas de REC qui clignote.
  await page.evaluate(() =>
    (window as unknown as { __emit: (e: string, p: unknown) => void }).__emit('enregistrement', { actif: false, pause: false, debut_ms: 0, notes: 0, titre: null }),
  );
  for (const section of ['notes', 'rappels', 'activite', 'reglages', 'a-propos']) {
    await page.evaluate((s) => window.kobiApp.aller(s), section);
    await page.waitForTimeout(1500);
    const sansFin = await page.evaluate(() =>
      document
        .getAnimations()
        .filter((a) => a.effect?.getTiming().iterations === Infinity && a.playState === 'running')
        .map((a) => {
          const cible = (a.effect as KeyframeEffect).target as Element;
          return { nom: (a as CSSAnimation).animationName, cible: cible.className, temps: getComputedStyle(cible).animationTimingFunction };
        })
        // Un clignotant en deux temps (`steps`) ne repeint que deux fois par cycle.
        .filter((a) => !a.temps.includes('steps')),
    );
    expect(sansFin, section).toEqual([]);
  }
  await expectNoErrors(errors);
});
