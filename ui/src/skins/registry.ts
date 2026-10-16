// Skins disponibles, par identifiant (réglage `skin` de config.toml).

import type { SkinFactory } from '../contract';
import { createFakeSkin } from './factice';
import { createKobiSkin } from './kobi';

export const SKINS: Record<string, SkinFactory> = {
  kobi: createKobiSkin,
  factice: createFakeSkin,
};

export const DEFAULT_SKIN = 'kobi';

export function skinFactory(id: string | null | undefined): SkinFactory {
  return (id && SKINS[id]) || SKINS[DEFAULT_SKIN]!;
}
