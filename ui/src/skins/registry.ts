// Skins disponibles, par identifiant (réglage `skin` de config.toml).

import type { SkinFactory } from '../contract';
import { createFakeSkin } from './factice';

export const SKINS: Record<string, SkinFactory> = {
  factice: createFakeSkin,
};

export const DEFAULT_SKIN = 'factice';

export function skinFactory(id: string | null | undefined): SkinFactory {
  return (id && SKINS[id]) || SKINS[DEFAULT_SKIN]!;
}
