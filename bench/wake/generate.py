"""Génère le jeu d'entraînement du mot-clé « Kobi » (KOB-24).

Positifs en français et en anglais : « Kobi », « hey Kobi » et « Kobi, <commande> »,
avec les voix Piper utilisables (plus de 900 locuteurs, dont 3 en français),
les voix Windows Hortense et Zira quand elles sont installées, cinq vitesses et
cinq hauteurs. Négatifs : mots proches (« copie », « robot », « Toby »…) et
phrases de réunion tirées des dialogues de `bench/corpus/`.

Les locuteurs sont répartis entre `train` et `test` : quelques voix entières
(`TEST_VOICES`) et un locuteur sur dix des voix multi-locuteurs vont en `test`,
pour mesurer la détection sur des voix jamais entendues.

Un négatif dont les phonèmes contiennent ceux de « Kobi » (homophone, comme
« Coby » en anglais) est écarté et listé dans le manifeste.

Sortie dans `<out>/` (défaut `~/Kobi-bench/wake/`) :

* `positive/<split>/*.wav` et `negative/<split>/*.wav` : 16 kHz mono ;
* `metadata.jsonl` : une ligne par clip ;
* `manifest.json` : comptes, voix, homophones écartés, SHA-256.

Usage : python bench/wake/generate.py [--out DIR] [--positives N] [--near N] [--phrases N]
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import random
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path

BENCH = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCH / "tts"))
import kobitts  # noqa: E402

# bench/corpus/generate.py porte le même nom que ce script : chargement par chemin.
_spec = importlib.util.spec_from_file_location("corpus_generate", BENCH / "corpus" / "generate.py")
corpus = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = corpus  # requis par dataclass
_spec.loader.exec_module(corpus)

LANGS = ("fr", "en")

VOICES = {
    "fr": [
        "fr_FR-tom-medium",
        "fr_FR-siwis-medium",
        "fr_FR-upmc-medium",
        "sapi:Microsoft Hortense Desktop",
    ],
    "en": [
        "en_US-ryan-medium",
        "en_US-amy-medium",
        "en_GB-alan-medium",
        "en_GB-jenny_dioco-medium",
        "en_US-libritts_r-medium",
        "sapi:Microsoft Zira Desktop",
    ],
}
# Écartées après écoute par Whisper : fr_FR-mls-medium, fr_FR-mls_1840-low et
# upmc#jessica (charabia), fr_FR-gilles-low (traîne d'une seconde après un mot
# isolé quand le bruit est à 0).
EXCLUDED_SPEAKERS = {"fr_FR-upmc-medium": {"jessica"}}

# Voix gardées pour le test, jamais entendues à l'entraînement.
TEST_VOICES = {"fr_FR-siwis-medium", "en_US-ryan-medium", "en_US-amy-medium"}
TEST_EVERY = 10  # un locuteur sur dix des voix multi-locuteurs en test

SPEEDS = (0.8, 0.9, 1.0, 1.15, 1.3)  # length_scale : < 1 plus rapide
PITCHES = (0.9, 0.95, 1.0, 1.05, 1.1)  # décalage de hauteur : pseudo-locuteurs

WAKE = "Kobi"
POSITIVES = {
    # forme -> (poids, textes) ; « command » = « Kobi, » suivi d'une commande
    "fr": {
        "wake": (3, ["Kobi."]),
        "hey": (3, ["Hey Kobi.", "Hé Kobi.", "Dis Kobi."]),
        "command": (4, [
            "tu peux noter ça ?",
            "crée un ticket.",
            "résume la réunion.",
            "c'est quoi la suite ?",
            "démarre la réunion.",
            "termine la réunion.",
            "pause.",
            "combien de tickets sont ouverts ?",
        ]),
    },
    "en": {
        "wake": (3, ["Kobi."]),
        "hey": (3, ["Hey Kobi.", "Okay Kobi."]),
        "command": (4, [
            "what's next?",
            "create a ticket.",
            "summarize the meeting.",
            "take a note.",
            "start the meeting.",
            "end the meeting.",
            "pause.",
            "how many issues are open?",
        ]),
    },
}
COMMA_PAUSE = 0.15  # entre « Kobi » et la commande

NEAR = {
    "fr": [
        "copie.", "Kobe.", "Coby.", "robot.", "Toby.", "cobaye.", "Gobi.",
        "Bobby.", "Kiki.", "colis.", "Kobo.", "Obi.", "copain.", "hobby.",
        "Fais une copie.", "Le robot.", "C'est Toby.", "Hé Bobby.",
        "Dis donc.", "Qui a bu ?", "Copie ça.", "On y va.",
    ],
    "en": [
        "copy.", "Kobe.", "Coby.", "robot.", "Toby.", "Gobi.", "Bobby.",
        "cookie.", "hobby.", "Colby.", "nobody.", "Kobo.", "Obi.", "go be.",
        "Make a copy.", "The robot.", "Hey Toby.", "Hey Bobby.",
        "Okay, buddy.", "Go big.", "Hey, copy that.", "Coat be.",
    ],
}


@dataclass
class Clip:
    id: str
    label: str  # positive | negative
    kind: str  # wake | hey | command | near | phrase
    lang: str
    text: str
    voice: str
    speaker: int | None
    speaker_name: str
    length_scale: float
    pitch: float
    split: str = ""
    extra: dict = field(default_factory=dict)


def speaker_pool(lang: str) -> list[tuple[str, int | None, str, str]]:
    """(voix, id, nom, split) pour chaque locuteur disponible."""
    pool = []
    installed = kobitts.sapi_voices()
    for voice in VOICES[lang]:
        if voice.startswith(kobitts.SAPI_PREFIX):
            if voice[len(kobitts.SAPI_PREFIX):] not in installed:
                print(f"{voice} absente de ce poste : ignorée", file=sys.stderr)
                continue
            speakers = {}
        else:
            speakers = kobitts.voice_speakers(voice)
        speakers = {n: i for n, i in speakers.items() if n not in EXCLUDED_SPEAKERS.get(voice, ())}
        if not speakers:
            split = "test" if voice in TEST_VOICES else "train"
            pool.append((voice, None, voice, split))
            continue
        for name, sid in sorted(speakers.items(), key=lambda kv: kv[1]):
            split = "test" if voice in TEST_VOICES or sid % TEST_EVERY == 0 else "train"
            pool.append((voice, sid, f"{voice}#{name}", split))
    return pool


def norm_phonemes(p: str) -> str:
    # « əʊ » (britannique) et « oʊ » (américain) sont la même voyelle ici.
    return p.replace("əʊ", "oʊ").replace("ː", "")


def homophones(lang: str) -> tuple[set[str], dict[str, str]]:
    """Phonèmes de « Kobi » pour toutes les voix, et négatifs proches qui en
    contiennent un (texte -> phonèmes), à écarter."""
    wake: set[str] = set()
    near_ph: dict[str, set[str]] = {t: set() for t in NEAR[lang]}
    piper = {other: [v for v in VOICES[other] if not v.startswith(kobitts.SAPI_PREFIX)] for other in LANGS}
    for other in LANGS:  # « Kobi » se dit aussi à l'anglaise en français
        for voice in piper[other]:
            wake.add(norm_phonemes(kobitts.phonemes([kobitts.spoken(WAKE + ".", other)], voice)[0]))
    for voice in piper[lang]:
        texts = [kobitts.spoken(t, lang) for t in NEAR[lang]]
        for t, p in zip(NEAR[lang], kobitts.phonemes(texts, voice)):
            near_ph[t].add(norm_phonemes(p))
    excluded = {}
    for t, ps in near_ph.items():
        for p in ps:
            if any(w in p.split() for w in wake):
                excluded[t] = p
    return wake, excluded


def plan(lang: str, n_pos: int, n_near: int, n_phrases: int, rng: random.Random) -> tuple[list[Clip], dict]:
    pool = speaker_pool(lang)
    rng.shuffle(pool)
    wake_ph, excluded = homophones(lang)
    near = [t for t in NEAR[lang] if t not in excluded]
    phrases = sorted({
        s.text
        for name in ("fr", "en", "mixte")
        for s in corpus.parse(corpus.DIALOGUES_DIR / f"{name}.txt")
        if s.lang == lang and WAKE.lower() not in s.text.lower()
    })

    forms = POSITIVES[lang]
    weighted = [k for k, (w, _) in forms.items() for _ in range(w)]
    clips: list[Clip] = []
    counter = 0

    seen: set[tuple] = set()

    def add(label: str, draw, i: int) -> None:
        """Ajoute un clip pour le locuteur i ; `draw` tire (forme, texte).
        Un même (locuteur, texte, vitesse, hauteur) donnerait un audio
        identique : on retire jusqu'à trouver une combinaison neuve."""
        nonlocal counter
        voice, sid, sname, split = pool[i % len(pool)]
        for _ in range(50):
            kind, text = draw()
            speed, pitch = rng.choice(SPEEDS), rng.choice(PITCHES)
            key = (voice, sid, text, speed, pitch)
            if key not in seen:
                break
        else:
            skipped[label] = skipped.get(label, 0) + 1
            return
        seen.add(key)
        clips.append(Clip(
            id=f"{lang}-{kind}-{counter:05d}",
            label=label, kind=kind, lang=lang, text=text,
            voice=voice, speaker=sid, speaker_name=sname,
            length_scale=speed, pitch=pitch, split=split,
        ))
        counter += 1

    def positive():
        kind = rng.choice(weighted)
        text = rng.choice(forms[kind][1])
        return kind, f"{WAKE}, {text}" if kind == "command" else text

    skipped: dict[str, int] = {}
    for i in range(n_pos):
        add("positive", positive, i)
    for i in range(n_near):
        add("negative", lambda: ("near", rng.choice(near)), n_pos + i)
    for i in range(n_phrases):
        add("negative", lambda: ("phrase", rng.choice(phrases)), n_pos + n_near + i)

    info = {
        "speakers": len(pool),
        "speakers_test": sum(1 for p in pool if p[3] == "test"),
        "voices": sorted({p[0] for p in pool}),
        "wake_phonemes": sorted(wake_ph),
        "near_used": near,
        "near_excluded_homophones": excluded,
        "meeting_phrases_available": len(phrases),
        "skipped_duplicates": skipped,
    }
    return clips, info


def render(clips: list[Clip]) -> list[bytes]:
    """Synthétise les clips ; « Kobi, <commande> » est assemblé à partir de
    « Kobi » et de la commande dits par la même voix, pour connaître la fin
    exacte du mot-clé."""
    utts: list[kobitts.Utterance] = []
    parts: list[tuple[int, ...]] = []
    for c in clips:
        if c.kind == "command":
            command = c.text.split(", ", 1)[1]
            first = len(utts)
            utts.append(kobitts.Utterance(kobitts.spoken(WAKE + ".", c.lang), c.voice, c.speaker, c.length_scale))
            utts.append(kobitts.Utterance(kobitts.spoken(command, c.lang), c.voice, c.speaker, c.length_scale))
            parts.append((first, first + 1))
        else:
            parts.append((len(utts),))
            utts.append(kobitts.Utterance(kobitts.spoken(c.text, c.lang), c.voice, c.speaker, c.length_scale))
    audio = kobitts.synthesize(utts)

    out = []
    for c, idx in zip(clips, parts):
        lead = kobitts.silence(0.1)
        if len(idx) == 2:
            wake = kobitts.trim(audio[idx[0]])
            rest = kobitts.trim(audio[idx[1]])
            pcm = lead + wake + kobitts.silence(COMMA_PAUSE) + rest + lead
            c.extra["wake_end"] = round(kobitts.duration(lead + wake), 3)
        else:
            pcm = lead + kobitts.trim(audio[idx[0]]) + lead
            if c.label == "positive":
                c.extra["wake_end"] = round(kobitts.duration(pcm) - 0.1, 3)
        out.append(kobitts.pitch_shift(pcm, c.pitch))
        if "wake_end" in c.extra:
            c.extra["wake_end"] = round(c.extra["wake_end"] / c.pitch, 3)
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--out", type=Path, default=kobitts.BENCH_HOME / "wake")
    parser.add_argument("--positives", type=int, default=1000, help="par langue")
    parser.add_argument("--near", type=int, default=600, help="négatifs proches par langue")
    parser.add_argument("--phrases", type=int, default=300, help="phrases de réunion par langue")
    parser.add_argument(
        "--threads", type=int, default=3,
        help="processeurs logiques utilisables, en priorité basse (défaut 3)",
    )
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    kobitts.limit_cpu(args.threads)

    rng = random.Random("kobi-wake")
    clips: list[Clip] = []
    infos = {}
    for lang in LANGS:
        lang_clips, infos[lang] = plan(lang, args.positives, args.near, args.phrases, rng)
        clips += lang_clips
        if infos[lang]["near_excluded_homophones"]:
            print(f"{lang} : homophones écartés : {infos[lang]['near_excluded_homophones']}")

    for sub in ("positive", "negative"):
        shutil.rmtree(args.out / sub, ignore_errors=True)
    audio = render(clips)

    meta_lines = []
    wav_hash = hashlib.sha256()
    counts: dict[str, int] = {}
    for c, pcm in zip(clips, audio):
        path = args.out / c.label / c.split / f"{c.id}.wav"
        kobitts.write_wav(path, pcm)
        wav_hash.update(c.id.encode() + hashlib.sha256(pcm).digest())
        for key in (c.label, f"{c.label}/{c.split}", f"{c.label}/{c.lang}", f"{c.label}/{c.kind}"):
            counts[key] = counts.get(key, 0) + 1
        meta_lines.append(json.dumps({
            "file": f"{c.label}/{c.split}/{c.id}.wav",
            "label": c.label,
            "kind": c.kind,
            "split": c.split,
            "lang": c.lang,
            "text": c.text,
            "spoken": kobitts.spoken(c.text, c.lang),
            "voice": c.speaker_name,
            "length_scale": c.length_scale,
            "pitch": c.pitch,
            "duration": round(kobitts.duration(pcm), 3),
            **c.extra,
        }, ensure_ascii=False))
    meta = args.out / "metadata.jsonl"
    meta.write_text("\n".join(meta_lines) + "\n", encoding="utf-8", newline="\n")

    manifest = {
        "generator": "bench/wake/generate.py",
        "piper_release": kobitts.PIPER_RELEASE,
        "voices_revision": kobitts.VOICES_REVISION,
        "sample_rate": kobitts.SAMPLE_RATE,
        "speeds": SPEEDS,
        "pitches": PITCHES,
        "counts": dict(sorted(counts.items())),
        "langs": infos,
        "sha256": {"metadata.jsonl": kobitts.sha256(meta), "wav": wav_hash.hexdigest()},
    }
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n"
    )
    for key, n in sorted(counts.items()):
        print(f"{key} : {n}")
    total = sum(kobitts.duration(p) for p in audio)
    print(f"{len(clips)} clips, {total / 60:.1f} min d'audio")


if __name__ == "__main__":
    main()
