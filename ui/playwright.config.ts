import { defineConfig, devices } from '@playwright/test';

// Tests de l'interface dans Chromium headless, sur le build de production
// servi par `vite preview` (aucune dépendance à la fenêtre native).
// Port réglable : deux arbres de travail peuvent tester en même temps.
const PORT = Number(process.env.KOBI_UI_PORT ?? 4173);

export default defineConfig({
  testDir: 'tests',
  fullyParallel: true,
  // Un seul navigateur : la machine est partagée avec des mesures de performance.
  workers: 1,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 820, height: 560 },
    trace: 'retain-on-failure',
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
