// Scène de démonstration du README (KOB-112) : Kobi pendant une réunion —
// REC, notes, on l'interpelle, il écoute, cherche, répond, propose un
// ticket confirmé d'un clic. `KOBI_DEMO=1 npx playwright test demo-gif`
// enregistre la vidéo dans test-results/ ; `scripts/demo-gif.sh` en fait
// docs/demo/kobi-demo.gif. Rien à vérifier : les comportements sont testés
// ailleurs (kobi.spec.ts, bulles.spec.ts…).
import { test, type Page } from '@playwright/test';
import { openUi } from './helpers';

test.skip(!process.env.KOBI_DEMO, 'démo à la demande (KOBI_DEMO=1)');
test.use({ video: { mode: 'on', size: { width: 820, height: 560 } }, viewport: { width: 820, height: 560 } });

const jouer = (page: Page, etat: string, ms: number) =>
  page.evaluate((e) => window.kobi.play(e as never), etat).then(() => page.waitForTimeout(ms));

test('démo : Kobi en réunion', async ({ page }) => {
  // Réunion déjà commencée, sans la fiche « Prévenez les participants ».
  await openUi(page, 'backdrop=1&reunion=Point%20hebdo&depuis=754&carte=0');
  await page.waitForTimeout(1500);
  await jouer(page, 'write', 3200);
  await jouer(page, 'listen', 3400);
  await jouer(page, 'think', 3600);
  await jouer(page, 'answer', 6000);
  await jouer(page, 'suggest', 2600);
  await jouer(page, 'done', 2400);
  await page.evaluate(() => window.kobi.setState('idle', {}));
  await page.waitForTimeout(1200);
});
