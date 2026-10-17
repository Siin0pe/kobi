// Réglages : choix du skin. Chaque skin se présente dans un aperçu vivant qui
// enchaîne ses 8 états ; le choix est enregistré (config.toml dans
// l'application) et appliqué tout de suite à la fenêtre de la mascotte.

import { STATES, STATE_LABELS } from './contract';
import { playDemo } from './demo';
import { loadSkin, saveSkin } from './skin-choice';
import { SKINS } from './skins/registry';
import { Stage } from './stage';
import './styles/fonts.css';
import './styles/stage.css';
import './styles/bubbles.css';
import './styles/reglages.css';

/** Skins proposés à l'utilisateur (le skin factice ne sert qu'aux tests). */
export const CHOICES: Array<{ id: string; name: string; idea: string }> = [
  { id: 'kobi', name: 'Kobi', idea: 'Un visage fait de lettres, vert acide. Le skin par défaut.' },
  { id: 'gribouille', name: 'Gribouille', idea: 'Un bonhomme à la craie au trait qui tremble.' },
  { id: 'mercure', name: 'Mercure', idea: 'Une goutte de chrome liquide, accent bleu électrique.' },
  { id: 'matrice', name: 'Matrice', idea: 'Un écran à points LED orange, très sobre.' },
  { id: 'gelee', name: 'Gelée', idea: 'Un bonbon de verre dépoli aux lumières colorées. Le plus gourmand.' },
  { id: 'essaim', name: 'Essaim', idea: 'Une nuée de 46 particules qui se réorganisent.' },
].filter((c) => c.id in SKINS);

const PREVIEW_MS = 2600;

const root = document.getElementById('reglages')!;
root.innerHTML = `
  <h1>Réglages</h1>
  <section aria-labelledby="skin-titre">
    <h2 id="skin-titre">Apparence de Kobi</h2>
    <div class="choix">
      <div class="liste" role="radiogroup" aria-labelledby="skin-titre"></div>
      <figure class="apercu" aria-hidden="true">
        <div class="cadre"></div>
        <figcaption></figcaption>
      </figure>
    </div>
    <p class="statut" role="status" aria-live="polite"></p>
  </section>`;

const list = root.querySelector<HTMLElement>('.liste')!;
const caption = root.querySelector<HTMLElement>('figcaption')!;
const status = root.querySelector<HTMLElement>('.statut')!;
const stage = new Stage(root.querySelector<HTMLElement>('.cadre')!, { backdrop: true, inFrame: true, announce: false });

let current = await loadSkin();
let previewed = '';
let step = 0;
let stopDemo: () => void = () => {};
let timer = 0;

function preview(id: string): void {
  if (id === previewed) return;
  previewed = id;
  stage.useSkin(SKINS[id]!);
  step = 0;
  window.clearInterval(timer);
  const next = (): void => {
    const state = STATES[step % STATES.length]!;
    stopDemo();
    stopDemo = playDemo(
      state,
      (s, d) => stage.setState(s, d),
      (d) => stage.update(d),
    );
    caption.textContent = `${CHOICES.find((c) => c.id === id)?.name} · ${STATE_LABELS[state]}`;
    step += 1;
  };
  next();
  timer = window.setInterval(next, PREVIEW_MS);
}

function render(): void {
  list.replaceChildren(
    ...CHOICES.map((c) => {
      const label = document.createElement('label');
      label.className = 'option';
      label.dataset.skin = c.id;
      const input = document.createElement('input');
      input.type = 'radio';
      input.name = 'skin';
      input.value = c.id;
      input.checked = c.id === current;
      input.addEventListener('change', () => void choose(c.id));
      input.addEventListener('focus', () => preview(c.id));
      label.addEventListener('pointerenter', () => preview(c.id));
      const text = document.createElement('span');
      text.innerHTML = `<strong></strong><small></small>`;
      text.querySelector('strong')!.textContent = c.name;
      text.querySelector('small')!.textContent = c.idea;
      label.append(input, text);
      return label;
    }),
  );
  list.addEventListener('pointerleave', () => preview(current));
}

async function choose(id: string): Promise<void> {
  try {
    await saveSkin(id);
    current = id;
    status.textContent = `${CHOICES.find((c) => c.id === id)?.name} est maintenant l’apparence de Kobi.`;
  } catch (e) {
    status.textContent = `Impossible d’enregistrer le choix : ${String(e)}`;
  }
  preview(id);
}

render();
preview(current);

declare global {
  interface Window {
    reglages: { choose(id: string): Promise<void>; current(): string };
  }
}
window.reglages = { choose, current: () => current };
