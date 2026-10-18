import { cpus } from 'node:os';
import { defineConfig, devices } from '@playwright/test';

// Tests de l'interface dans Chromium headless, sur le build de production
// servi par `vite preview` (aucune dépendance à la fenêtre native).
// Port réglable : deux arbres de travail peuvent tester en même temps.
const PORT = Number(process.env.KOBI_UI_PORT ?? 4173);

// Navigateurs en parallèle (KOB-172) : les trois quarts des cœurs, de 2 à 6 ;
// `KOBI_PW_WORKERS` l'impose (1 pour une mesure de performance au calme).
const WORKERS = Number(process.env.KOBI_PW_WORKERS) || Math.min(6, Math.max(2, Math.floor(cpus().length * 0.75)));

export default defineConfig({
  testDir: 'tests',
  fullyParallel: true,
  workers: WORKERS,
  // Délais pensés pour un PC chargé (un cargo build en même temps) : un test
  // qui passe ne les attend jamais.
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 820, height: 560 },
    // Ni trace ni vidéo en vérification : elles cassent sous manque de mémoire
    // (« End of central directory record signature not found »). Pour
    // déboguer : KOBI_PW_TRACE=1. Les vidéos des tickets : KOBI_PW_VIDEOS=1.
    trace: process.env.KOBI_PW_TRACE ? 'retain-on-failure' : 'off',
    video: 'off',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 820, height: 560 } } }],
  webServer: {
    command: `npm run build && npx vite preview --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}`,
    // Jamais le serveur d'un autre arbre : il servirait un autre build.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
