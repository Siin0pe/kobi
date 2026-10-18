import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const page = (fichier: string) => fileURLToPath(new URL(fichier, import.meta.url));

// Tauri sert `dist/` ; en développement, le serveur Vite sur le port 1420.
// Trois pages : la mascotte, le Carnet (KOB-140) et les Réglages.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: ['es2022', 'safari15'],
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        main: page('./index.html'),
        carnet: page('./carnet.html'),
        reglages: page('./reglages.html'),
      },
    },
  },
});
