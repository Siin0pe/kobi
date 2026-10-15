"""Synthèse vocale locale et reproductible pour les jeux de benchmark de Kobi.

Outillage commun à `bench/corpus/` (KOB-12) et `bench/wake/` (KOB-24).

* Moteur : Piper (binaire officiel 2023.11.14-2, licence MIT), voix
  `rhasspy/piper-voices` épinglées sur la révision `v1.0.0`.
* Tout est téléchargé à la demande dans `~/Kobi-bench/` (hors dépôt) et
  vérifié par SHA-256.
* Bruit du générateur à 0 (`noise_scale` et `noise_w`) : la sortie est
  identique octet pour octet d'une exécution à l'autre, ce qui rend les
  horodatages de référence exacts.
* Sortie : PCM 16 bits, 16 kHz, mono (rééchantillonnage sinc fenêtré, en
  numpy, déterministe).

Dépendances : Python 3.10+ et numpy.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import wave
import zipfile
from dataclasses import dataclass
from pathlib import Path

import numpy as np

SAMPLE_RATE = 16_000

BENCH_HOME = Path(os.environ.get("KOBI_BENCH_HOME", Path.home() / "Kobi-bench"))
TOOLS_DIR = BENCH_HOME / "tools"
VOICES_DIR = BENCH_HOME / "voices"

PIPER_RELEASE = "2023.11.14-2"
PIPER_ATTEMPTS = 3
PIPER_ARCHIVES = {
    # plate-forme Python -> (archive, sha256)
    "win32": (
        "piper_windows_amd64.zip",
        "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea",
    ),
}

VOICES_REVISION = "v1.0.0"
VOICES_URL = "https://huggingface.co/rhasspy/piper-voices/resolve/" + VOICES_REVISION

# nom -> (chemin dans piper-voices, sha256 .onnx, sha256 .onnx.json)
VOICES = {
    "fr_FR-tom-medium": (
        "fr/fr_FR/tom/medium",
        "bf65074ccdeeeeaa832e75edb1c0a513c01c9a972bdf085ff8a6e71ea234fd41",
        "2f7f885ad5a0aad802e3cc24e4f57239febdcb142b4876de5d238094674361cc",
    ),
    "fr_FR-siwis-medium": (
        "fr/fr_FR/siwis/medium",
        "641d1ab097da2b81128c076810edb052b385decc8be3381814802a64a73baf99",
        "39479916c2db192b5ac9764daddd0c744d83e023ad890c6976c0633ae4df8959",
    ),
    "fr_FR-upmc-medium": (
        "fr/fr_FR/upmc/medium",
        "9abb3800c199148897a9ed64e100d224f3de83579f100044174ad19418f1786f",
        "e8636ec15dfd5d72db37a02cb5320a20f2b8d339f2a0e4337da64c58a33a5868",
    ),
    "fr_FR-gilles-low": (
        "fr/fr_FR/gilles/low",
        "5cd711846720e261c2a176f6924c198a7424d0a75dd4b0a5357a5fb9cb739285",
        "5a47cc0789e91267d17666bbec842dd92950669271a09023eb6970ee364cf88a",
    ),
    "fr_FR-mls-medium": (
        "fr/fr_FR/mls/medium",
        "0ed223f78466917f2bae05ee90096ce69ab1fdeb251f55590d0e7422d234e162",
        "252b0b0a6e4cc4949e23eccb956f9c779986c32f934f2f7e2191e5fdc2edca61",
    ),
    "en_US-ryan-medium": (
        "en/en_US/ryan/medium",
        "abf4c274862564ed647ba0d2c47f8ee7c9b717d27bdad9219100eb310db4047a",
        "44034c056cb15681b2ad494307c7f3f2e4499d1253c700c711fa0a4607ffe78d",
    ),
    "en_US-amy-medium": (
        "en/en_US/amy/medium",
        "b3a6e47b57b8c7fbe6a0ce2518161a50f59a9cdd8a50835c02cb02bdd6206c18",
        "95a23eb4d42909d38df73bb9ac7f45f597dbfcde2d1bf9526fdeaf5466977d77",
    ),
    "en_GB-alan-medium": (
        "en/en_GB/alan/medium",
        "0a309668932205e762801f1efc2736cd4b0120329622adf62be09e56339d3330",
        "c0f0d124e5895c00e7c03b35dcc8287f319a6998a365b182deb5c8e752ee8c1e",
    ),
    "en_GB-jenny_dioco-medium": (
        "en/en_GB/jenny_dioco/medium",
        "469c630d209e139dd392a66bf4abde4ab86390a0269c1e47b4e5d7ce81526b01",
        "a9a7a93a317c9a3cb6563e37eb057df9ef09c06188a8a4341b0fcb58cba54dd4",
    ),
    "en_US-libritts_r-medium": (
        "en/en_US/libritts_r/medium",
        "10bb85e071d616fcf4071f369f1799d0491492ab3c5d552ec19fb548fac13195",
        "b471dc60d2d8335e819c393d196d6fbf792817f40051257b269878505bc9afb3",
    ),
}


# Graphies données au moteur pour qu'il prononce bien « Kobi » (/kobi/). Les
# voix anglaises lisent « Kobi » « Ko-bye » ; « Koby » donne le bon son.
PRONUNCIATION = {
    "en": [(re.compile(r"\bKobi\b"), "Koby")],
}


def spoken(text: str, lang: str) -> str:
    """Texte à donner au moteur ; la référence garde le texte d'origine."""
    for pattern, repl in PRONUNCIATION.get(lang, []):
        text = pattern.sub(repl, text)
    return text


@dataclass(frozen=True)
class Utterance:
    """Une phrase à synthétiser."""

    text: str
    voice: str
    speaker: int | None = None  # uniquement pour les voix multi-locuteurs
    length_scale: float = 1.0


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _download(url: str, dest: Path, expected_sha: str) -> None:
    if dest.exists() and sha256(dest) == expected_sha:
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    print(f"téléchargement {url}", file=sys.stderr)
    with urllib.request.urlopen(url) as r, open(tmp, "wb") as f:
        shutil.copyfileobj(r, f)
    got = sha256(tmp)
    if got != expected_sha:
        tmp.unlink()
        raise RuntimeError(f"{dest.name} : sha256 {got}, attendu {expected_sha}")
    tmp.replace(dest)


def piper_exe() -> Path:
    """Installe Piper si besoin et renvoie le chemin de l'exécutable."""
    exe = TOOLS_DIR / "piper" / ("piper.exe" if sys.platform == "win32" else "piper")
    if exe.exists():
        return exe
    if sys.platform not in PIPER_ARCHIVES:
        raise RuntimeError(
            f"pas d'archive Piper épinglée pour {sys.platform} : installer Piper "
            f"{PIPER_RELEASE} dans {exe.parent}"
        )
    name, sha = PIPER_ARCHIVES[sys.platform]
    archive = TOOLS_DIR / name
    _download(
        f"https://github.com/rhasspy/piper/releases/download/{PIPER_RELEASE}/{name}",
        archive,
        sha,
    )
    with zipfile.ZipFile(archive) as z:
        z.extractall(TOOLS_DIR)
    return exe


def voice_model(name: str) -> Path:
    """Télécharge la voix si besoin et renvoie le chemin du modèle .onnx."""
    rel, sha_model, sha_config = VOICES[name]
    model = VOICES_DIR / f"{name}.onnx"
    _download(f"{VOICES_URL}/{rel}/{name}.onnx", model, sha_model)
    _download(f"{VOICES_URL}/{rel}/{name}.onnx.json", model.with_suffix(".onnx.json"), sha_config)
    return model


def voice_speakers(name: str) -> dict[str, int]:
    """Locuteurs d'une voix multi-locuteurs ({} si un seul)."""
    config = voice_model(name).with_suffix(".onnx.json")
    return json.loads(config.read_text(encoding="utf-8")).get("speaker_id_map", {})


def resample(x: np.ndarray, sr_in: int, sr_out: int = SAMPLE_RATE, taps: int = 32) -> np.ndarray:
    """Rééchantillonnage par sinc fenêtré (Kaiser, beta 8.6), en float64."""
    if sr_in == sr_out:
        return x.astype(np.float64)
    ratio = sr_in / sr_out
    cutoff = min(1.0, 1.0 / ratio) * 0.95  # garde une marge sous Nyquist
    n_out = int(np.floor(len(x) / ratio))
    window = np.kaiser(2 * taps + 1, 8.6)
    offsets = np.arange(-taps + 1, taps + 1)
    padded = np.concatenate([np.zeros(taps), x.astype(np.float64), np.zeros(taps + 1)])
    out = np.empty(n_out)
    for lo in range(0, n_out, 8192):  # par blocs pour borner la mémoire
        t = np.arange(lo, min(lo + 8192, n_out)) * ratio  # positions dans l'entrée
        idx = np.floor(t).astype(np.int64)[:, None] + offsets[None, :]
        d = t[:, None] - idx  # distance en échantillons d'entrée
        kernel = cutoff * np.sinc(cutoff * d) * window[
            np.clip(np.round(d + taps).astype(np.int64), 0, 2 * taps)
        ]
        kernel[np.abs(d) > taps] = 0.0
        out[lo : lo + len(t)] = (padded[idx + taps] * kernel).sum(axis=1)
    return out


def _to_16k(src: Path) -> bytes:
    """Lit un WAV mono 16 bits de Piper et renvoie du PCM s16le 16 kHz."""
    with wave.open(str(src), "rb") as w:
        assert w.getnchannels() == 1 and w.getsampwidth() == 2
        sr = w.getframerate()
        x = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    y = resample(x.astype(np.float64), sr)
    return np.clip(np.round(y), -32768, 32767).astype("<i2").tobytes()


def synthesize(utterances: list[Utterance]) -> list[bytes]:
    """Synthétise chaque phrase et renvoie son PCM s16le 16 kHz mono.

    Un seul process Piper par couple (voix, vitesse) : le modèle n'est chargé
    qu'une fois.
    """
    exe = piper_exe()
    results: list[bytes | None] = [None] * len(utterances)
    groups: dict[tuple[str, float], list[int]] = {}
    for i, u in enumerate(utterances):
        groups.setdefault((u.voice, u.length_scale), []).append(i)

    with tempfile.TemporaryDirectory(prefix="kobitts-") as tmp:
        for (voice, length_scale), indices in groups.items():
            model = voice_model(voice)
            todo = indices
            for attempt in range(PIPER_ATTEMPTS):
                lines = []
                for i in todo:
                    u = utterances[i]
                    line = {"text": u.text, "output_file": str(Path(tmp) / f"{i}.wav")}
                    if u.speaker is not None:
                        # Piper plante si on passe speaker_id à une voix mono-locuteur.
                        line["speaker_id"] = u.speaker
                    lines.append(json.dumps(line, ensure_ascii=False))
                proc = subprocess.run(
                    [str(exe), "-m", str(model), "--json-input", "-q",
                     "--noise_scale", "0", "--noise_w", "0",
                     "--length_scale", str(length_scale), "--sentence_silence", "0"],
                    input="\n".join(lines) + "\n",
                    encoding="utf-8",
                    capture_output=True,
                )
                if proc.returncode == 0:
                    break
                # Piper plante parfois sous Windows (0xC0000409) sans raison
                # reproductible. Les phrases sont écrites dans l'ordre : on
                # garde les fichiers finis, on jette le dernier (peut-être
                # tronqué) et on relance sur le reste. La sortie étant
                # déterministe, le résultat ne change pas.
                done = [i for i in todo if (Path(tmp) / f"{i}.wav").exists()]
                if done:
                    (Path(tmp) / f"{done[-1]}.wav").unlink()
                    done.pop()
                todo = [i for i in todo if i not in set(done)]
                print(
                    f"piper {voice} : code {proc.returncode:#x}, nouvelle tentative "
                    f"pour {len(todo)} phrases",
                    file=sys.stderr,
                )
            else:
                raise RuntimeError(f"piper {voice} : échec après {PIPER_ATTEMPTS} tentatives")
            for i in indices:
                results[i] = _to_16k(Path(tmp) / f"{i}.wav")
    assert all(r is not None for r in results)
    return results  # type: ignore[return-value]


def write_wav(path: Path, pcm: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm)


def silence(seconds: float) -> bytes:
    return bytes(2 * round(seconds * SAMPLE_RATE))


def duration(pcm: bytes) -> float:
    return len(pcm) / 2 / SAMPLE_RATE
