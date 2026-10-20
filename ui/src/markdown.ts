// Mise en forme légère d'une réponse en flux (KOB-163) : paragraphes, listes
// à puces ou numérotées, titres, blocs de code, `code` et **gras** en ligne.
// Le texte arrive caractère par caractère (frappe) : on ne relit jamais ce qui
// est posé, et un début de ligne ambigu (« - », « 1 », « `` ») attend la
// suite avant d'être classé. Chaque mot reste un élément `.w` qui arrive avec
// son petit mouvement ; les chemins et adresses longs se coupent (CSS).

import { HIGHLIGHT } from './typewriter';

/** Début de ligne qui peut encore devenir une marque (puce, numéro, clôture, titre). */
const PENDING = [/^\s*$/, /^\s*[-*•]$/, /^\s*\d{1,3}[.)]?$/, /^\s*`{1,2}$/, /^#{1,6}$/];
const FENCE = /^\s*```/;
const BULLET = /^\s*[-*•]\s/;
const NUMBER = /^\s*\d{1,3}[.)]\s/;
const HEADING = /^#{1,6}\s/;

export class Markdown {
  /** Caractères du texte déjà posés. */
  private done = 0;
  private text = '';
  private block: HTMLElement | null = null;
  private list: HTMLElement | null = null;
  /** Conteneur en ligne courant (bloc, `code` ou gras). */
  private inline: HTMLElement | null = null;
  private word: HTMLElement | null = null;
  private lineStart = true;
  private fenced = false;
  /** Reste de la ligne d'ouverture d'un bloc de code (langage), ignoré. */
  private fenceInfo = false;
  private code = false;
  private strong = false;

  constructor(
    private readonly root: Element,
    private readonly animate: () => boolean,
  ) {}

  /** Texte posé jusqu'ici (début de `text` à chaque appel). */
  get rendered(): string {
    return this.text.slice(0, this.done);
  }

  /** Pose la suite de `text` ; `final` : plus rien ne viendra, tout se pose. */
  feed(text: string, final: boolean): void {
    this.text = text;
    let i = this.done;
    while (i < text.length) {
      if (this.lineStart) {
        const next = this.startLine(i, final);
        if (next < 0) break;
        i = next;
        continue;
      }
      const c = text[i]!;
      if (c === '\n') {
        this.closeWord();
        if (this.fenced && !this.fenceInfo) this.inline!.append('\n');
        this.lineStart = true;
        this.fenceInfo = false;
        i++;
        continue;
      }
      if (this.fenceInfo) {
        i++;
        continue;
      }
      if (this.fenced) {
        this.inline!.append(c);
        i++;
        continue;
      }
      if (c === '`') {
        this.closeWord();
        this.toggleInline('code');
        i++;
        continue;
      }
      if (c === '*') {
        if (i + 1 >= text.length && !final) break;
        if (text[i + 1] === '*') {
          this.closeWord();
          this.toggleInline('strong');
          i += 2;
          continue;
        }
      }
      if (/\s/.test(c)) {
        this.closeWord();
        this.target().append(c);
      } else {
        if (!this.word) {
          this.word = document.createElement('span');
          this.word.className = this.animate() ? 'w w-in' : 'w';
          this.target().append(this.word);
        }
        this.word.textContent += c;
      }
      i++;
    }
    this.done = i;
    if (final && this.done >= text.length) this.closeWord();
  }

  /** Classe une ligne qui commence ; renvoie où reprendre, ou -1 pour attendre la suite. */
  private startLine(i: number, final: boolean): number {
    const text = this.text;
    const nl = text.indexOf('\n', i);
    const line = text.slice(i, nl < 0 ? text.length : nl);
    if (nl < 0 && !final && (this.fenced ? /^\s*`{0,2}$/.test(line) : PENDING.some((p) => p.test(line)))) return -1;
    this.lineStart = false;
    if (this.fenced) {
      if (FENCE.test(line)) {
        this.fenced = false;
        this.closeBlock();
        this.fenceInfo = true;
        return i + line.indexOf('```') + 3;
      }
      return i;
    }
    if (FENCE.test(line)) {
      this.closeBlock();
      this.closeList();
      const pre = document.createElement('pre');
      const code = document.createElement('code');
      pre.append(code);
      this.root.append(pre);
      this.block = pre;
      this.inline = code;
      this.fenced = true;
      this.fenceInfo = true;
      return i + line.indexOf('```') + 3;
    }
    if (line.trim() === '') {
      // Ligne vide : fin du paragraphe. Une liste « aérée » (ligne vide entre
      // deux éléments, comme Claude les écrit) continue : c'est la prochaine
      // ligne qui n'en est pas un élément qui la ferme. Sinon, chaque élément
      // ouvrait sa propre liste et tous s'affichaient « 1. » (KOB-247).
      this.closeBlock();
      return nl < 0 ? text.length : i + line.length;
    }
    const bullet = BULLET.exec(line) ?? NUMBER.exec(line);
    if (bullet) {
      const tag = BULLET.test(line) ? 'UL' : 'OL';
      this.closeBlock();
      if (this.list?.tagName !== tag) {
        this.closeList();
        this.list = document.createElement(tag.toLowerCase());
        // Une liste numérotée commence à son premier numéro (« 3. » après un
        // paragraphe continue à 3).
        const premier = tag === 'OL' ? Number.parseInt(line, 10) : 1;
        if (premier > 1) this.list.setAttribute('start', String(premier));
        this.root.append(this.list);
      }
      this.open(document.createElement('li'), this.list);
      return i + bullet[0].length;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      this.closeBlock();
      this.closeList();
      const h = document.createElement('p');
      h.className = 'md-titre';
      this.open(h, this.root);
      return i + heading[0].length;
    }
    if (this.block?.tagName === 'P' && !this.block.classList.contains('md-titre')) {
      // Retour à la ligne dans un paragraphe.
      this.inline!.append(document.createElement('br'));
      return i;
    }
    this.closeBlock();
    this.closeList();
    this.open(document.createElement('p'), this.root);
    return i;
  }

  private open(block: HTMLElement, parent: Element): void {
    parent.append(block);
    this.block = block;
    this.inline = block;
    this.code = false;
    this.strong = false;
  }

  private target(): HTMLElement {
    if (!this.inline) this.open(document.createElement('p'), this.root);
    return this.inline!;
  }

  private toggleInline(kind: 'code' | 'strong'): void {
    const on = kind === 'code' ? !this.code : !this.strong;
    if (kind === 'code') this.code = on;
    else this.strong = on;
    if (on) {
      const node = document.createElement(kind);
      if (kind === 'code') node.className = 'md-code';
      this.target().append(node);
      this.inline = node;
    } else {
      this.inline = (this.inline?.parentElement as HTMLElement | null) ?? this.block;
    }
  }

  private closeWord(): void {
    if (this.word && HIGHLIGHT.test(this.word.textContent ?? '')) this.word.classList.add('hl');
    this.word = null;
  }

  private closeBlock(): void {
    this.closeWord();
    this.block = null;
    this.inline = null;
    this.code = false;
    this.strong = false;
  }

  private closeList(): void {
    this.list = null;
  }
}
