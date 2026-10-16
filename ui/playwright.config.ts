import { defineConfig, devices } from '@playwright/test';

// Tests de l'interface dans Chromium headless, sur le build de production
// servi par `vite preview` (aucune dépendance à la fenêtre native).
export default defineConfig({
  testDir: 'tests',
  fullyParallel: true,
  workers: 2,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: 'http://127.0.0.1:4173',
    viewport: { width: 820, height: 560 },
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 820, height: 560 } } }],
  webServer: {
    command: 'npm run build && npm run preview -- --host 127.0.0.1',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
