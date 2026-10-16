"""Génère le jeu d'entraînement du mot-clé « Kobi » (KOB-24, variété ajoutée par KOB-26).

Positifs en français et en anglais : « Kobi », « hey Kobi » et « Kobi, <commande> »,
avec les voix Piper utilisables (plus de 1 000 locuteurs, dont 4 en français),
les voix Windows Hortense et Zira quand elles sont installées, cinq vitesses,
cinq hauteurs et trois niveaux de bruit du générateur Piper. Négatifs : mots
proches (« copie », « robot », « Toby »…) et phrases de réunion tirées des
dialogues de `bench/corpus/`.

Le bruit du générateur rend les voix moins uniformes, mais Piper le tire sans
graine : seuls les clips sans bruit sont reproductibles octet pour octet.
Un clip dont la durée par caractère dépasse 2,5 fois la médiane de sa langue
(traîne ou babillage d'une voix « low ») est écarté.

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
        "fr_FR-gilles-low",
        "sapi:Microsoft Hortense Desktop",
    ],
    "en": [
        "en_US-ryan-medium",
        "en_US-amy-medium",
        "en_GB-alan-medium",
        "en_GB-jenny_dioco-medium",
        "en_US-libritts_r-medium",
        "en_GB-vctk-medium",
        "en_GB-cori-medium",
        "en_US-kristin-medium",
        "en_US-norman-medium",
        "en_US-john-medium",
        "sapi:Microsoft Zira Desktop",
    ],
}
# Écartées après écoute par Whisper : fr_FR-mls-medium, fr_FR-mls_1840-low et
# upmc#jessica (charabia). fr_FR-gilles-low traîne d'une seconde après un mot
# isolé quand le bruit est à 0 : ses clips trop longs sont filtrés (voir
# MAX_DURATION_RATIO).
EXCLUDED_SPEAKERS = {"fr_FR-upmc-medium": {"jessica"}}

# Voix gardées pour le test, jamais entendues à l'entraînement. tom (AGPL) et
# ryan (CC BY-NC-SA) n'entrent ainsi dans aucun modèle entraîné.
TEST_VOICES = {"fr_FR-tom-medium", "en_US-ryan-medium", "en_US-amy-medium"}
TEST_EVERY = 10  # un locuteur sur dix des voix multi-locuteurs en test

SPEEDS = (0.8, 0.9, 1.0, 1.15, 1.3)  # length_scale : < 1 plus rapide
PITCHES = (0.9, 0.95, 1.0, 1.05, 1.1)  # décalage de hauteur : pseudo-locuteurs
# Bruit du générateur Piper (noise_scale, noise_w) ; (0, 0) = déterministe.
NOISES = ((0.0, 0.0), (0.333, 0.333), (0.667, 0.8))
MAX_DURATION_RATIO = 2.5

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
    noise: tuple[float, float] = (0.0, 0.0)
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
        Une même combinaison (locuteur, texte, vitesse, hauteur, bruit) n'est
        tirée qu'une fois."""
        nonlocal counter
        voice, sid, sname, split = pool[i % len(pool)]
        sapi = voice.startswith(kobitts.SAPI_PREFIX)
        for _ in range(50):
            kind, text = draw()
            speed, pitch = rng.choice(SPEEDS), rng.choice(PITCHES)
            noise = NOISES[0] if sapi else rng.choice(NOISES)  # SAPI : pas de bruit
            key = (voice, sid, text, speed, pitch, noise)
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
            length_scale=speed, pitch=pitch, noise=noise, split=split,
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


def render(clips: list[Clip]) -> tuple[list[bytes], list[float]]:
    """Synthétise les clips ; « Kobi, <commande> » est assemblé à partir de
    « Kobi » et de la commande dits par la même voix, pour connaître la fin
    exacte du mot-clé. Renvoie aussi, pour chaque clip, la durée par
    caractère de sa première partie, ramenée à la vitesse 1."""
    utts: list[kobitts.Utterance] = []
    parts: list[tuple[int, ...]] = []

    def utt(c: Clip, text: str) -> kobitts.Utterance:
        return kobitts.Utterance(
            kobitts.spoken(text, c.lang), c.voice, c.speaker, c.length_scale,
            noise_scale=c.noise[0], noise_w=c.noise[1],
        )

    for c in clips:
        if c.kind == "command":
            command = c.text.split(", ", 1)[1]
            parts.append((len(utts), len(utts) + 1))
            utts += [utt(c, WAKE + "."), utt(c, command)]
        else:
            parts.append((len(utts),))
            utts.append(utt(c, c.text))
    audio = kobitts.synthesize(utts)

    out, ratios = [], []
    for c, idx in zip(clips, parts):
        lead = kobitts.silence(0.1)
        first = kobitts.trim(audio[idx[0]])
        ratios.append(kobitts.duration(first) / c.length_scale / len(utts[idx[0]].text))
        if len(idx) == 2:
            rest = kobitts.trim(audio[idx[1]])
            pcm = lead + first + kobitts.silence(COMMA_PAUSE) + rest + lead
            c.extra["wake_end"] = round(kobitts.duration(lead + first), 3)
        else:
            pcm = lead + first + lead
            if c.label == "positive":
                c.extra["wake_end"] = round(kobitts.duration(pcm) - 0.1, 3)
        out.append(kobitts.pitch_shift(pcm, c.pitch))
        if "wake_end" in c.extra:
            c.extra["wake_end"] = round(c.extra["wake_end"] / c.pitch, 3)
    return out, ratios


def outliers(clips: list[Clip], ratios: list[float]) -> list[bool]:
    """Clips anormalement longs pour leur texte (traîne, babillage) : durée
    par caractère au-delà de MAX_DURATION_RATIO fois la médiane de la langue."""
    medians = {}
    for lang in {c.lang for c in clips}:
        values = sorted(r for c, r in zip(clips, ratios) if c.lang == lang)
        medians[lang] = values[len(values) // 2]
    return [r > MAX_DURATION_RATIO * medians[c.lang] for c, r in zip(clips, ratios)]


def synth_key(c: Clip) -> tuple:
    return (c.voice, c.length_scale, c.noise)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--out", type=Path, default=kobitts.BENCH_HOME / "wake")
    parser.add_argument("--positives", type=int, default=1500, help="par langue")
    parser.add_argument("--near", type=int, default=600, help="négatifs proches par langue")
    parser.add_argument("--phrases", type=int, default=300, help="phrases de réunion par langue")
    parser.add_argument("--batch", type=int, default=400, help="clips synthétisés par lot")
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

    # Par lots, écrits au fur et à mesure, pour borner la mémoire. Les clips
    # sont triés par réglage de synthèse : un lot ne relance Piper que pour
    # quelques voix.
    order = sorted(range(len(clips)), key=lambda i: synth_key(clips[i]))
    ratios: list[float] = [0.0] * len(clips)
    digests: list[bytes] = [b""] * len(clips)
    durations: list[float] = [0.0] * len(clips)
    for b in range(0, len(order), args.batch):
        idx = order[b : b + args.batch]
        audio, batch_ratios = render([clips[i] for i in idx])
        for i, pcm, r in zip(idx, audio, batch_ratios):
            c = clips[i]
            kobitts.write_wav(args.out / c.label / c.split / f"{c.id}.wav", pcm)
            ratios[i], digests[i], durations[i] = r, hashlib.sha256(pcm).digest(), kobitts.duration(pcm)
        print(f"  {min(b + args.batch, len(order))}/{len(order)} clips", flush=True)

    dropped: dict[str, int] = {}
    kept = []
    for i, (c, bad) in enumerate(zip(clips, outliers(clips, ratios))):
        if bad:
            (args.out / c.label / c.split / f"{c.id}.wav").unlink()
            dropped[c.speaker_name] = dropped.get(c.speaker_name, 0) + 1
        else:
            kept.append(i)
    if dropped:
        print(f"clips trop longs écartés : {dropped}")

    meta_lines = []
    wav_hash = hashlib.sha256()
    counts: dict[str, int] = {}
    for i in kept:
        c = clips[i]
        wav_hash.update(c.id.encode() + digests[i])
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
            "noise_scale": c.noise[0],
            "noise_w": c.noise[1],
            "duration": round(durations[i], 3),
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
        "noises": NOISES,
        "dropped_too_long": dropped,
        "voice_licenses": {v: kobitts.VOICE_LICENSES.get(v, "?") for lang in LANGS for v in VOICES[lang]},
        "counts": dict(sorted(counts.items())),
        "langs": infos,
        "sha256": {"metadata.jsonl": kobitts.sha256(meta), "wav": wav_hash.hexdigest()},
    }
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n"
    )
    for key, n in sorted(counts.items()):
        print(f"{key} : {n}")
    total = sum(durations[i] for i in kept)
    print(f"{len(kept)} clips, {total / 60:.1f} min d'audio")


if __name__ == "__main__":
    main()
