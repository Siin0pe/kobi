// Repos sans boucle (KOB-240) : au repos, avec les réglages par défaut,
// aucune boucle JavaScript ne fait avancer les animations à chaque image, et
// le style n'est pas recalculé en continu, dans la mascotte comme dans l'app.
// Écran simulé à 60, 144 et 240 Hz, et réglage à 15 images/s : sous la
// fréquence de l'écran, le métronome (econome.ts) tenait toutes les
// animations et les faisait avancer lui-même à chaque image, d'où un repeint
// permanent sous Windows.
import { expect, test, type Page } from '@playwright/test';
import { fakeTauri } from './app-fausse-coque';
import { mockTauri, openUi } from './helpers';

const REPOS_MS = 5000;
/**
 * Images demandées au plus en `REPOS_MS` : les gestes de la vie au repos
 * (lettre qui se brouille) en prennent quelques-unes ; une boucle par image
 * en prendrait 300 à 60 Hz.
 */
const IMAGES_MAX = 30;

/** Boucle rAF et recalculs de style pendant `REPOS_MS` de repos. */
async function activite(page: Page, hz: number): Promise<{ images: number; styles: number }> {
  await page.evaluate((hz) => window.kobiAnimations.ecran(hz), hz);
  // Démarrage passé (mesure de l'écran, premières entrées).
  await page.waitForTimeout(1500);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Performance.enable');
  const styles = async () => (await cdp.send('Performance.getMetrics')).metrics.find((m) => m.name === 'RecalcStyleCount')!.value;
  const s0 = await styles();
  await page.evaluate(() => ((window as unknown as { __images: number }).__images = 0));
  await page.waitForTimeout(REPOS_MS);
  const images = await page.evaluate(() => (window as unknown as { __images: number }).__images);
  return { images, styles: (await styles()) - s0 };
}

test.beforeEach(async ({ page }) => {
  // Compte les images demandées par la page (boucle partagée, métronome…).
  await page.addInitScript(() => {
    const w = window as unknown as { __images: number };
    w.__images = 0;
    const demander = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = (cb) =>
      demander((t) => {
        w.__images++;
        cb(t);
      });
  });
});

// KOB-200 : à 15, 30 et 60 images/s réglées sous la fréquence de l'écran,
// les animations passent en escalier, toujours sans boucle par image.
const CAS: Array<{ hz: number; fps?: 15 | 30 | 60 }> = [
  { hz: 60 },
  { hz: 144 },
  { hz: 240 },
  { hz: 144, fps: 15 },
  { hz: 60, fps: 15 },
  { hz: 60, fps: 30 },
  { hz: 144, fps: 60 },
];

for (const { hz, fps } of CAS) {
  const cas = `écran ${hz} Hz${fps ? `, ${fps} images/s` : ''}`;

  test(`mascotte au repos, ${cas} : ni boucle par image, ni style recalculé en continu`, async ({ page }) => {
    await mockTauri(page, fps ? { reglage_animations: { fps, economie: false } } : {});
    await openUi(page, '');
    const { images, styles } = await activite(page, hz);
    // Avant le correctif, à 144 Hz : ~180 à 310 images et ~4 700 recalculs en 5 s.
    expect(images, `${images} images demandées en ${REPOS_MS} ms`).toBeLessThanOrEqual(IMAGES_MAX);
    expect(styles, `${styles} recalculs de style en ${REPOS_MS} ms`).toBeLessThanOrEqual(300);
  });

  test(`app au repos, ${cas} : ni boucle par image, ni style recalculé en continu`, async ({ page }) => {
    await page.addInitScript(fakeTauri);
    if (fps) await page.addInitScript((fps) => window.addEventListener('DOMContentLoaded', () => window.kobiAnimations?.poser({ fps })), fps);
    await page.goto('/app.html');
    await page.waitForLoadState('networkidle');
    const { images, styles } = await activite(page, hz);
    // Avant le correctif, à 144 Hz : ~300 images et ~950 recalculs en 5 s.
    expect(images, `${images} images demandées en ${REPOS_MS} ms`).toBeLessThanOrEqual(IMAGES_MAX);
    expect(styles, `${styles} recalculs de style en ${REPOS_MS} ms`).toBeLessThanOrEqual(200);
  });
}
