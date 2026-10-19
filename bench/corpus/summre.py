"""Corpus français réel tiré de SUMM-RE (réunions transcrites à la main).

SUMM-RE (Linagora, Université d'Aix-Marseille, licence CC BY-SA 4.0) : des
réunions en français de 3 ou 4 personnes, un micro par participant, les
splits `dev` et `test` transcrits et alignés à la main. Ce script prend des
extraits de réunions du split `test` et les met au format des corpus de
`kobi-bench` :

* `moi.wav` : la piste du participant qui parle le plus dans l'extrait ;
* `eux.wav` : la somme des pistes des autres participants (le son de
  l'ordinateur en visio) ;
* `reference.jsonl` / `reference.txt` : la transcription manuelle de SUMM-RE,
  phrase par phrase, avec ses mots horodatés.

Les pistes sont gardées brutes : mesurée sur ces réunions, la voix des
voisins arrive dans chaque micro au niveau du bruit de fond (à 4 dB près), 20
à 30 dB sous la parole propre. `--attenuation N` atténue de N dB chaque micro
hors de sa parole annotée, si un autre extrait en avait besoin.

Dans le texte de référence, les rires (`@`), passages inaudibles (`*`), mots
coupés (`addi-`) et hésitations (`euh`, `mh`…) sont retirés ; `parce_que`
devient `parce que`. Le texte brut reste dans le champ `text_brut`.

Usage : python bench/corpus/summre.py [--out DIR] [--cache DIR]
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
from pathlib import Path

import fsspec
import numpy as np
import pyarrow.parquet as pq
import soundfile as sf
import soxr

DATASET = "linagora/SUMM-RE"
# Révision du jeu épinglée pour que deux extractions donnent le même corpus.
REVISION = "47cb4ec3437ef5497b6092b8c6d1e0b5b1d86dc0"
SHARD_URL = (
    "https://huggingface.co/datasets/" + DATASET + "/resolve/{rev}/data/test/"
    "test-{i:05d}-of-00028.parquet"
)
SAMPLE_RATE = 16000

# Réunion -> fichiers parquet qui contiennent ses pistes. Lieu (dernière
# lettre) : Z Zoom, D à domicile, H studio, L laboratoire.
MEETINGS = {
    "012c_EBPZ": (1, 2),
    "013c_EAPD": (2,),
    "019a_EARD": (4, 5),
    "039b_EDDH": (7, 8),
    "070a_EBRL": (17, 18),
}
WINDOW = 300.0  # durée visée d'un extrait, en secondes
WINDOW_SLACK = 30.0  # l'extrait commence et finit dans un silence de toutes les pistes
SKIP_START = 60.0  # le début des réunions (présentations, consignes) est évité
PAD = 0.25  # marge autour de la parole annotée avant atténuation

FILLERS = {"euh", "heu", "hum", "hm", "mh", "mhm", "mmh", "mm", "hmm"}
DROP_RE = re.compile(r"^[@*]+$|-$")


def clean(text: str) -> str:
    words = []
    for w in text.replace("_", " ").split():
        if DROP_RE.search(w) or w.lower() in FILLERS:
            continue
        words.append(w)
    return " ".join(words)


def fetch(meeting: str, cache: Path) -> dict[str, tuple[np.ndarray, list]]:
    """Pistes 16 kHz d'une réunion, gardées dans `cache` après le premier
    téléchargement."""
    cache.mkdir(parents=True, exist_ok=True)
    found = sorted(cache.glob(f"{meeting}_*.npy"))
    if not found:
        fs = fsspec.filesystem("https")
        for i in MEETINGS[meeting]:
            with fs.open(SHARD_URL.format(rev=REVISION, i=i), block_size=8 << 20) as f:
                table = pq.ParquetFile(f).read(
                    columns=["meeting_id", "audio_id", "audio", "segments"]
                )
            for row in table.to_pylist():
                if row["meeting_id"] != meeting:
                    continue
                x, sr = sf.read(io.BytesIO(row["audio"]["bytes"]), dtype="float32", always_2d=True)
                y = soxr.resample(x.mean(axis=1), sr, SAMPLE_RATE).astype(np.float32)
                np.save(cache / f"{row['audio_id']}.npy", y)
                (cache / f"{row['audio_id']}.json").write_text(
                    json.dumps(row["segments"], ensure_ascii=False), encoding="utf-8"
                )
        found = sorted(cache.glob(f"{meeting}_*.npy"))
    return {
        p.stem.rsplit("_", 1)[1]: (
            np.load(p),
            json.loads(p.with_suffix(".json").read_text(encoding="utf-8")),
        )
        for p in found
    }


def pick_window(tracks: dict[str, tuple[np.ndarray, list]]) -> tuple[float, float]:
    """Extrait d'environ `WINDOW` secondes, bordé par des silences de toutes
    les pistes, qui contient le plus de mots."""
    segs = [s for _, ss in tracks.values() for s in ss]
    length = min(len(x) for x, _ in tracks.values()) / SAMPLE_RATE

    def free(t: float) -> bool:
        return all(not (s["start"] - PAD <= t <= s["end"] + PAD) for s in segs)

    def words(a: float, b: float) -> int:
        return sum(
            len(clean(s["transcript"]).split()) for s in segs if a <= s["start"] and s["end"] <= b
        )

    best = None
    t = SKIP_START
    while t + WINDOW - WINDOW_SLACK < length:
        if free(t):
            ends = [
                e / 2
                for e in range(
                    int(2 * (t + WINDOW - WINDOW_SLACK)),
                    int(2 * min(t + WINDOW + WINDOW_SLACK, length)),
                )
            ]
            ends = [e for e in ends if free(e)]
            if ends:
                end = min(ends, key=lambda e: abs(e - t - WINDOW))
                score = words(t, end)
                if best is None or score > best[0]:
                    best = (score, t, end)
        t += 0.5
    if best is None:
        raise SystemExit("aucun extrait bordé de silences")
    return best[1], best[2]


def gate(x: np.ndarray, segs: list, att_db: float) -> np.ndarray:
    gain = np.full(len(x), 10 ** (-att_db / 20), dtype=np.float32)
    for s in segs:
        a = max(0, int((s["start"] - PAD) * SAMPLE_RATE))
        b = min(len(x), int((s["end"] + PAD) * SAMPLE_RATE))
        gain[a:b] = 1.0
    # Rampe de 20 ms pour éviter les clics.
    k = int(0.02 * SAMPLE_RATE)
    gain = np.convolve(gain, np.ones(k) / k, mode="same").astype(np.float32)
    return x * gain


def write_wav(path: Path, x: np.ndarray) -> None:
    sf.write(path, np.clip(x, -1, 1), SAMPLE_RATE, subtype="PCM_16")


def overlapping(reference: list[dict], source: str) -> list[dict]:
    """Phrases de `source` qui chevauchent une autre phrase de la même piste."""
    rs = [r for r in reference if r["source"] == source]
    return [
        r
        for r in rs
        if any(o is not r and o["start"] < r["end"] and r["start"] < o["end"] for o in rs)
    ]


def merge_overlaps(utterances: list[dict]) -> list[dict]:
    """Fusionne les phrases qui se chevauchent sur une même piste, mots rangés
    par heure de début : une piste mélangée ne peut rendre que ce flux, et la
    référence par phrase mettrait les mots des deux locuteurs dans le désordre."""
    groups: list[list[dict]] = []
    for r in sorted(utterances, key=lambda r: r["start"]):
        if groups and r["start"] < max(g["end"] for g in groups[-1]):
            groups[-1].append(r)
        else:
            groups.append([r])
    merged = []
    for g in groups:
        if len(g) == 1:
            merged.append(g[0])
            continue
        words = sorted((w for r in g for w in r["words"]), key=lambda w: (w["start"], w["end"]))
        merged.append(
            {
                **g[0],
                "end": max(r["end"] for r in g),
                "locuteur": "+".join(dict.fromkeys(r["locuteur"] for r in g)),
                "text": " ".join(w["text"] for w in words),
                "text_brut": " / ".join(r["text_brut"] for r in g),
                "words": words,
            }
        )
    return merged


def fmt(t: float) -> str:
    return f"{int(t // 60):02d}:{t % 60:06.3f}"


def build(meeting: str, cache: Path, out: Path, att_db: float) -> dict:
    tracks = fetch(meeting, cache)
    start, end = pick_window(tracks)

    def inside(ss):
        return [s for s in ss if start <= s["start"] and s["end"] <= end]

    counts = {
        spk: sum(len(clean(s["transcript"]).split()) for s in inside(ss))
        for spk, (_, ss) in tracks.items()
    }
    moi = max(counts, key=counts.get)
    a, b = int(start * SAMPLE_RATE), int(end * SAMPLE_RATE)

    mix = {"moi": np.zeros(b - a, np.float32), "eux": np.zeros(b - a, np.float32)}
    reference = []
    for spk, (x, ss) in tracks.items():
        source = "moi" if spk == moi else "eux"
        mix[source] += (gate(x, ss, att_db) if att_db else x)[a:b]
        for s in inside(ss):
            text = clean(s["transcript"])
            if not text:
                continue
            words = [
                {
                    "source": source,
                    "start": round(w["start"] - start, 3),
                    "end": round(w["end"] - start, 3),
                    "text": c,
                }
                for w in s["words"]
                if (c := clean(w["word"]))
            ]
            reference.append(
                {
                    "start": round(s["start"] - start, 3),
                    "end": round(s["end"] - start, 3),
                    "source": source,
                    "lang": "fr",
                    "kobi": False,
                    "locuteur": spk,
                    "text": text,
                    "text_brut": s["transcript"],
                    "words": words,
                }
            )
    overlapped = sum(len(r["words"]) for r in overlapping(reference, "eux"))
    reference = [r for r in reference if r["source"] == "moi"] + merge_overlaps(
        [r for r in reference if r["source"] == "eux"]
    )
    reference.sort(key=lambda r: (r["start"], r["source"]))

    dest = out / meeting
    dest.mkdir(parents=True, exist_ok=True)
    peak = max(float(np.abs(v).max()) for v in mix.values())
    scale = 0.95 / peak if peak > 0.95 else 1.0
    files = {}
    for source, x in mix.items():
        path = dest / f"{source}.wav"
        write_wav(path, x * scale)
        files[path.name] = path
    jsonl = dest / "reference.jsonl"
    jsonl.write_text(
        "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in reference),
        encoding="utf-8",
        newline="\n",
    )
    files[jsonl.name] = jsonl
    txt = dest / "reference.txt"
    txt.write_text(
        "".join(
            f"[{fmt(r['start'])} - {fmt(r['end'])}] {r['source']} ({r['locuteur']}) {r['text']}\n"
            for r in reference
        ),
        encoding="utf-8",
        newline="\n",
    )
    files[txt.name] = txt

    speech = {
        src: sum(r["end"] - r["start"] for r in reference if r["source"] == src)
        for src in ("moi", "eux")
    }
    return {
        "source_start_s": start,
        "duration_s": round(end - start, 3),
        "moi": moi,
        "eux": sorted(s for s in tracks if s != moi),
        "segments": len(reference),
        "eux_overlap_words": overlapped,
        "words": sum(len(r["text"].split()) for r in reference),
        "speech_s": {k: round(v, 1) for k, v in speech.items()},
        "sha256": {n: hashlib.sha256(p.read_bytes()).hexdigest() for n, p in files.items()},
    }


def main() -> None:
    home = Path.home() / "Kobi-bench"
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--out", type=Path, default=home / "corpus-reel-fr")
    ap.add_argument("--cache", type=Path, default=home / "cache" / "summre")
    ap.add_argument("--attenuation", type=float, default=0.0)
    ap.add_argument("meetings", nargs="*", default=list(MEETINGS))
    args = ap.parse_args()

    manifest = {
        "generator": "bench/corpus/summre.py",
        "dataset": DATASET,
        "revision": REVISION,
        "split": "test",
        "license": "CC BY-SA 4.0",
        "credit": "SUMM-RE, LINAGORA et Laboratoire Parole et Langage (Aix-Marseille Université), ANR-20-CE23-0017",
        "sample_rate": SAMPLE_RATE,
        "attenuation_db": args.attenuation,
        "meetings": {},
    }
    for m in args.meetings:
        info = build(m, args.cache, args.out, args.attenuation)
        manifest["meetings"][m] = info
        print(
            f"{m} : {info['duration_s']:.1f} s, {info['segments']} segments, {info['words']} mots, "
            f"parole moi {info['speech_s']['moi']} s / eux {info['speech_s']['eux']} s",
            file=sys.stderr,
        )
    (args.out / "manifest.json").write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    main()
