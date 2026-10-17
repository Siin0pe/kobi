import { defineConfig } from 'vite';

// Tauri sert `dist/` ; en développement, le serveur Vite sur le port 1420.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: ['es2022', 'safari15'],
    assetsInlineLimit: 0,
    // Deux pages : la mascotte et les réglages.
    rollupOptions: { input: { main: 'index.html', reglages: 'reglages.html' } },
  },
});
