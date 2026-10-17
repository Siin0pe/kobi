// Skins disponibles, par identifiant (réglage `skin` de config.toml).

import type { SkinFactory } from '../contract';
import { createFakeSkin } from './factice';
import { createGeleeSkin } from './gelee';
import { createGribouilleSkin } from './gribouille';
import { createKobiSkin } from './kobi';
import { createMatriceSkin } from './matrice';
import { createMercureSkin } from './mercure';

export const SKINS: Record<string, SkinFactory> = {
  kobi: createKobiSkin,
  gribouille: createGribouilleSkin,
  mercure: createMercureSkin,
  matrice: createMatriceSkin,
  gelee: createGeleeSkin,
  factice: createFakeSkin,
};

export const DEFAULT_SKIN = 'kobi';

export function skinFactory(id: string | null | undefined): SkinFactory {
  return (id && SKINS[id]) || SKINS[DEFAULT_SKIN]!;
}
