import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const page = (fichier: string) => fileURLToPath(new URL(fichier, import.meta.url));

// Tauri sert `dist/` ; en développement, le serveur Vite sur le port 1420.
// Pages : la mascotte, l'app Kobi (KOB-159 : notes, notes en direct,
// rappels, activité, réglages, accueil) et le menu du clic droit (KOB-202).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: ['es2022', 'safari15'],
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        main: page('./index.html'),
        app: page('./app.html'),
        menu: page('./menu.html'),
      },
    },
  },
});
