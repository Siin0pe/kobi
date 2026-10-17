"""Génère le corpus synthétique de Kobi (KOB-12).

Pour chaque dialogue de `dialogues/*.txt`, écrit dans `<out>/<nom>/` :

* `moi.wav` et `eux.wav` : une piste par interlocuteur, 16 kHz mono, même
  durée, silence sur une piste quand l'autre parle ;
* `reference.jsonl` : une ligne par phrase avec début, fin (secondes), piste,
  langue, interpellation de Kobi et texte exact ;
* `reference.txt` : la même chose, lisible.

Et `<out>/manifest.json` : voix, versions, durées et SHA-256 des fichiers.

Usage : python bench/corpus/generate.py [--out DIR] [dialogue ...]
"""

from __future__ import annotations

import argparse
import json
import random
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tts"))
import kobitts  # noqa: E402

HERE = Path(__file__).resolve().parent
DIALOGUES_DIR = HERE / "dialogues"

TRACKS = ("moi", "eux")

# (piste, langue) -> voix Piper. Une voix masculine pour moi, féminine pour eux.
VOICES = {
    ("moi", "fr"): "fr_FR-tom-medium",
    ("moi", "en"): "en_US-ryan-medium",
    ("eux", "fr"): "fr_FR-siwis-medium",
    ("eux", "en"): "en_US-amy-medium",
}

LEAD_IN = 1.0  # silence au début et à la fin de l'enregistrement
SENTENCE_GAP = (0.15, 0.35)  # entre deux phrases d'un même tour
TURN_GAP = (0.35, 0.9)  # entre deux tours
KOBI_PAUSE = 3.0  # après une interpellation : Kobi répond dans sa bulle

TURN_RE = re.compile(r"^(moi|eux)(@kobi)?\s*(?:\[(fr|en)\])?\s*:\s*(.+)$")
SENTENCE_RE = re.compile(r"(?<=[.?!])\s+")


@dataclass
class Sentence:
    track: str
    lang: str
    kobi: bool
    text: str
    new_turn: bool


def parse(path: Path) -> list[Sentence]:
    """Lit un dialogue. Voir le README pour le format."""
    lang = None
    out: list[Sentence] = []
    for n, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("@lang "):
            lang = line.split()[1]
            continue
        m = TURN_RE.match(line)
        if not m or lang is None:
            raise SystemExit(f"{path.name}:{n} : ligne invalide : {line}")
        track, kobi, turn_lang, text = m.groups()
        for i, s in enumerate(SENTENCE_RE.split(text)):
            out.append(Sentence(track, turn_lang or lang, bool(kobi), s, i == 0))
    return out


def build(name: str, out_dir: Path) -> dict:
    sentences = parse(DIALOGUES_DIR / f"{name}.txt")
    clips = kobitts.synthesize([
        kobitts.Utterance(kobitts.spoken(s.text, s.lang), VOICES[(s.track, s.lang)])
        for s in sentences
    ])
    rng = random.Random(name)  # pauses pseudo-aléatoires mais reproductibles

    tracks = {t: bytearray(kobitts.silence(LEAD_IN)) for t in TRACKS}
    reference = []
    t = LEAD_IN
    for i, (s, clip) in enumerate(zip(sentences, clips)):
        if i > 0:
            prev = sentences[i - 1]
            if s.new_turn and prev.kobi:
                gap = KOBI_PAUSE
            elif s.new_turn:
                gap = rng.uniform(*TURN_GAP)
            else:
                gap = rng.uniform(*SENTENCE_GAP)
            t += gap
        clip = kobitts.trim(clip)
        start = t
        t += kobitts.duration(clip)
        for track in TRACKS:
            buf = tracks[track]
            buf.extend(bytes(2 * round(start * kobitts.SAMPLE_RATE) - len(buf)))
            if track == s.track:
                buf.extend(clip)
        reference.append({
            "start": round(start, 3),
            "end": round(t, 3),
            "source": s.track,
            "lang": s.lang,
            "kobi": s.kobi,
            "text": s.text,
        })

    total = round((t + LEAD_IN) * kobitts.SAMPLE_RATE)
    dest = out_dir / name
    files = {}
    for track in TRACKS:
        buf = tracks[track]
        buf.extend(bytes(2 * total - len(buf)))
        path = dest / f"{track}.wav"
        kobitts.write_wav(path, bytes(buf))
        files[path.name] = path

    jsonl = dest / "reference.jsonl"
    jsonl.write_text(
        "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in reference),
        encoding="utf-8",
        newline="\n",
    )
    txt = dest / "reference.txt"
    txt.write_text(
        "".join(
            f"[{fmt(r['start'])} - {fmt(r['end'])}] {r['source']} ({r['lang']})"
            f"{' [kobi]' if r['kobi'] else ''} {r['text']}\n"
            for r in reference
        ),
        encoding="utf-8",
        newline="\n",
    )
    files[jsonl.name] = jsonl
    files[txt.name] = txt

    words = sum(len(r["text"].split()) for r in reference)
    return {
        "duration_s": round(total / kobitts.SAMPLE_RATE, 2),
        "sentences": len(reference),
        "words": words,
        "kobi_sentences": sum(r["kobi"] for r in reference),
        "voices": sorted({VOICES[(s.track, s.lang)] for s in sentences}),
        "sha256": {n: kobitts.sha256(p) for n, p in files.items()},
    }


def fmt(seconds: float) -> str:
    m, s = divmod(seconds, 60)
    return f"{int(m):02d}:{s:06.3f}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--out", type=Path, default=kobitts.BENCH_HOME / "corpus")
    parser.add_argument("dialogues", nargs="*", help="noms (défaut : tous)")
    parser.add_argument(
        "--threads", type=int, default=4,
        help="processeurs logiques utilisables, en priorité basse (défaut 4)",
    )
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    kobitts.limit_cpu(args.threads)

    names = args.dialogues or sorted(p.stem for p in DIALOGUES_DIR.glob("*.txt"))
    manifest = {
        "generator": "bench/corpus/generate.py",
        "piper_release": kobitts.PIPER_RELEASE,
        "voices_revision": kobitts.VOICES_REVISION,
        "sample_rate": kobitts.SAMPLE_RATE,
        "voices": {f"{t}/{l}": v for (t, l), v in VOICES.items()},
        "dialogues": {},
    }
    for name in names:
        info = build(name, args.out)
        manifest["dialogues"][name] = info
        print(
            f"{name} : {info['duration_s']:.1f} s, {info['sentences']} phrases, "
            f"{info['words']} mots, {info['kobi_sentences']} adressées à Kobi"
        )
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


if __name__ == "__main__":
    main()
