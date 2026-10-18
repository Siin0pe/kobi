// Arrête un processus lancé en `detached` et tous ses enfants (vite preview
// lance esbuild…). Sous Windows, `process.kill(-pid)` n'existe pas.

import { spawnSync } from 'node:child_process';

export function stopTree(child) {
  if (process.platform === 'win32') spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
  else process.kill(-child.pid);
}
