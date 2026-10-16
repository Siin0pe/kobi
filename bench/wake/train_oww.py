"""Entraîne un modèle openWakeWord « Kobi » sur le jeu de `bench/wake/generate.py` (KOB-26).

Chaîne openWakeWord : audio 16 kHz → `melspectrogram.onnx` (32 bandes, trames de
10 ms, transformée x/10 + 2) → `embedding_model.onnx` (fenêtres de 76 trames au
pas de 8, soit un vecteur de 96 valeurs toutes les 80 ms) → modèle « Kobi » sur
les 16 derniers vecteurs (environ 2 s) → score entre 0 et 1. Les deux premiers
modèles sont ceux d'openWakeWord v0.5.1 ; seul le dernier est entraîné ici.

Augmentation, tirée à chaque passage : position du clip après 1,8 à 3 s de
fond (le modèle voit environ 2 s), réverbération synthétique (RT60 de 0,15 à 0,8 s), bruit blanc, rose ou
brun, ou brouhaha de réunion (phrases négatives d'entraînement superposées),
rapport signal sur bruit de 0 à 25 dB, gain.

Évaluation : positifs et négatifs de `test` (voix jamais entendues), propres et
bruités ; interpellations et faux déclenchements sur le corpus synthétique ;
faux déclenchements par heure sur le jeu de validation d'openWakeWord (11 h de
parole et de bruit, features précalculées).

Sortie dans `<out>/` : `kobi.onnx` (entrée `x` [n, 16, 96], sortie `score`
[n, 1]), `metrics.json`, `card.json` (données, voix et licences, empreintes).

Dépendances (hors dépôt) : numpy, onnxruntime, onnx, torch.
Usage : python bench/wake/train_oww.py [--data DIR] [--out DIR] [--device cuda|cpu] [--threads 3]
"""

from __future__ import annotations

import argparse
import json
import random
import sys
import time
import wave
from pathlib import Path

import numpy as np

BENCH = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCH / "tts"))
import kobitts  # noqa: E402

SR = 16_000
FRAMES = 16  # vecteurs vus par le modèle
EMB = 96
OWW_DIR = kobitts.BENCH_HOME / "models" / "openwakeword"
OWW_RELEASE = "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1"
OWW_FILES = {
    "melspectrogram.onnx": "ba2b0e0f8b7b875369a2c89cb13360ff53bac436f2895cced9f479fa65eb176f",
    "embedding_model.onnx": "70d164290c1d095d1d4ee149bc5e00543250a7316b59f31d056cff7bd3075c1f",
}
VALIDATION = (
    "https://huggingface.co/datasets/davidscripka/openwakeword_features/resolve/main/"
    "validation_set_features.npy",
    "a56a8a0f8e0efb91900acc6de4c0cdf4c564842e8475a7d49b36c039e17a690f",
)

# Fenêtre positive : la fin du mot-clé tombe dans les POS_AFTER dernières
# secondes vues par le modèle.
POS_AFTER = (0.0, 0.35)
DEBOUNCE = 1.5  # s entre deux détections
# Deux fenêtres négatives voisines partagent 15 vecteurs sur 16 : on n'en
# garde qu'une sur NEG_STRIDE, et les fenêtres sont stockées en float16, pour
# tenir en quelques centaines de Mo sur un poste partagé.
NEG_STRIDE = 3


def emb_end_time(j: int) -> float:
    """Fin, en secondes, de l'audio vu par le vecteur j (trames 8j à 8j+75 ;
    le mel donne N/160 - 3 trames, la trame k couvre 640 échantillons)."""
    return (160 * (8 * j + 75) + 640) / SR


# ---------------------------------------------------------------- features


class Features:
    """Mel puis embeddings, exactement comme openWakeWord (hors flux)."""

    def __init__(self, threads: int):
        import onnxruntime as ort

        for name, sha in OWW_FILES.items():
            kobitts._download(f"{OWW_RELEASE}/{name}", OWW_DIR / name, sha)
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        opts.inter_op_num_threads = 1
        cpu = ["CPUExecutionProvider"]
        self.mel = ort.InferenceSession(str(OWW_DIR / "melspectrogram.onnx"), opts, providers=cpu)
        self.emb = ort.InferenceSession(str(OWW_DIR / "embedding_model.onnx"), opts, providers=cpu)

    def melspec(self, audio: np.ndarray) -> np.ndarray:
        x = audio.astype(np.float32)[None, :]
        return np.squeeze(self.mel.run(None, {"input": x})[0]) / 10 + 2  # (trames, 32)

    def embeddings(self, audio: np.ndarray, first: int = 0, last: int | None = None) -> np.ndarray:
        """Vecteurs first..last (inclus) d'un clip int16 ; (n, 96)."""
        spec = self.melspec(audio)
        n = (spec.shape[0] - 76) // 8 + 1
        last = n - 1 if last is None else min(last, n - 1)
        if n <= 0 or last < first:
            return np.zeros((0, EMB), np.float32)
        windows = np.stack([spec[8 * j : 8 * j + 76] for j in range(first, last + 1)])
        out = []
        for b in range(0, len(windows), 256):
            out.append(self.emb.run(None, {"input_1": windows[b : b + 256, :, :, None].astype(np.float32)})[0])
        return np.concatenate(out).reshape(-1, EMB)


# ------------------------------------------------------------ augmentation


def read_wav(path: Path) -> np.ndarray:
    with wave.open(str(path), "rb") as w:
        assert w.getframerate() == SR and w.getnchannels() == 1
        return np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float32)


def colored_noise(n: int, kind: str, rng: np.random.Generator) -> np.ndarray:
    white = rng.standard_normal(n)
    if kind == "white":
        return white
    spec = np.fft.rfft(white)
    f = np.maximum(np.fft.rfftfreq(n), 1.0 / n)
    spec /= np.sqrt(f) if kind == "pink" else f  # rose : 1/f, brun : 1/f²
    out = np.fft.irfft(spec, n)
    return out / (out.std() + 1e-9)


def synthetic_rir(rng: np.random.Generator) -> np.ndarray:
    """Réponse impulsionnelle de salle : chemin direct, réflexions précoces,
    queue exponentielle de RT60 tiré entre 0,15 et 0,8 s."""
    rt60 = rng.uniform(0.15, 0.8)
    n = int(rt60 * SR)
    t = np.arange(n) / SR
    rir = rng.standard_normal(n) * np.exp(-6.9 * t / rt60) * rng.uniform(0.05, 0.25)
    rir[: rng.integers(SR // 500, SR // 200)] = 0.0  # pré-délai de 2 à 5 ms
    for _ in range(rng.integers(3, 8)):
        rir[rng.integers(SR // 500, SR // 30)] += rng.uniform(-0.5, 0.5)
    rir[0] = max(1.0, 1.1 * np.abs(rir[1:]).max())  # le chemin direct domine
    return rir / np.sqrt(np.sum(rir**2))


class Augmenter:
    def __init__(self, babble_clips: list[np.ndarray], seed: int):
        self.babble_clips = babble_clips
        self.rng = np.random.default_rng(seed)

    def babble(self, n: int) -> np.ndarray:
        out = np.zeros(n)
        for _ in range(self.rng.integers(1, 4)):  # 1 à 3 voix superposées
            stream = []
            while sum(len(s) for s in stream) < n:
                stream.append(self.babble_clips[self.rng.integers(len(self.babble_clips))])
            s = np.concatenate(stream)
            start = self.rng.integers(0, len(s) - n + 1)
            out += s[start : start + n] / (s.std() + 1e-9)
        return out / (out.std() + 1e-9)

    def background(self, n: int) -> np.ndarray:
        kind = self.rng.choice(["white", "pink", "brown", "babble", "babble"])
        return self.babble(n) if kind == "babble" else colored_noise(n, kind, self.rng)

    def __call__(self, clip: np.ndarray, pre: float, post: float = 0.4) -> tuple[np.ndarray, int]:
        """Place le clip après `pre` s de fond ; renvoie l'audio int16 et
        l'indice d'échantillon où commence le clip."""
        rng = self.rng
        x = clip.astype(np.float64)
        if rng.random() < 0.5:
            x = np.convolve(x, synthetic_rir(rng))[: len(x) + SR // 4]
        start = int(pre * SR)
        total = start + len(x) + int(post * SR)
        audio = np.zeros(total)
        audio[start : start + len(x)] = x
        speech_rms = np.sqrt(np.mean(x**2)) + 1e-9
        if rng.random() < 0.85:
            snr = rng.uniform(0, 25)
            audio += self.background(total) * speech_rms / 10 ** (snr / 20)
        else:
            audio += rng.standard_normal(total) * speech_rms * 1e-3  # quasi-silence
        peak = np.abs(audio).max() + 1e-9
        audio *= rng.uniform(0.05, 0.9) * 32767 / peak
        return np.clip(audio, -32768, 32767).astype(np.int16), start


def quiet_pad(clip: np.ndarray, pre: float, post: float, seed: int) -> tuple[np.ndarray, int]:
    """Clip propre, précédé et suivi d'un souffle à peine audible (le silence
    numérique pur fausse le mel)."""
    rng = np.random.default_rng(seed)
    start, total = int(pre * SR), int(pre * SR) + len(clip) + int(post * SR)
    audio = rng.standard_normal(total) * 3
    audio[start : start + len(clip)] += clip
    return np.clip(audio, -32768, 32767).astype(np.int16), start


# ----------------------------------------------------------------- données


def load_metadata(data: Path) -> list[dict]:
    return [json.loads(line) for line in open(data / "metadata.jsonl", encoding="utf-8")]


def windows_of(emb: np.ndarray) -> np.ndarray:
    """Toutes les fenêtres de 16 vecteurs d'une suite (n, 96) → (n-15, 16, 96)."""
    if len(emb) < FRAMES:
        return np.zeros((0, FRAMES, EMB), np.float32)
    return np.lib.stride_tricks.sliding_window_view(emb, (FRAMES, EMB))[:, 0]


def build_training_set(rows, data, feats, aug, n_aug_pos, n_aug_neg, n_noise, rng):
    xs, ys = [], []
    t0 = time.time()
    for k, r in enumerate(rows):
        clip = read_wav(data / r["file"])
        positive = r["label"] == "positive"
        for _ in range(n_aug_pos if positive else n_aug_neg):
            pre = rng.uniform(1.8, 3.0) if positive else rng.uniform(0.5, 2.5)
            audio, start = aug(clip, pre)
            if positive:
                wake_end = start / SR + r["wake_end"]
                js = [
                    j for j in range(FRAMES - 1, 400)
                    if POS_AFTER[0] <= emb_end_time(j) - wake_end <= POS_AFTER[1]
                ]
                emb = feats.embeddings(audio, js[0] - FRAMES + 1, js[-1])
                w = windows_of(emb)
                xs.append(w.astype(np.float16))
                ys.append(np.ones(len(w), np.float32))
                # Avant le mot-clé : fond seul, négatif.
                js_before = [j for j in range(400) if emb_end_time(j) < start / SR + 0.05 and j >= FRAMES - 1]
                if js_before:
                    wb = windows_of(feats.embeddings(audio, 0, js_before[-1]))[::NEG_STRIDE]
                    xs.append(wb.astype(np.float16))
                    ys.append(np.zeros(len(wb), np.float32))
            else:
                w = windows_of(feats.embeddings(audio))[::NEG_STRIDE]
                xs.append(w.astype(np.float16))
                ys.append(np.zeros(len(w), np.float32))
        if (k + 1) % 500 == 0:
            print(f"  {k + 1}/{len(rows)} clips, {time.time() - t0:.0f} s", flush=True)
    for _ in range(n_noise):  # fond seul, sans parole cible
        audio, _ = aug(np.zeros(int(rng.uniform(0.5, 2.0) * SR)), rng.uniform(1.0, 3.0))
        w = windows_of(feats.embeddings(audio))[::NEG_STRIDE]
        xs.append(w.astype(np.float16))
        ys.append(np.zeros(len(w), np.float32))
    return np.concatenate(xs), np.concatenate(ys)


# ------------------------------------------------------------------ modèle


def make_model():
    import torch.nn as nn

    return nn.Sequential(
        nn.Flatten(),
        nn.Linear(FRAMES * EMB, 128),
        nn.LayerNorm(128),
        nn.ReLU(),
        nn.Dropout(0.2),
        nn.Linear(128, 128),
        nn.LayerNorm(128),
        nn.ReLU(),
        nn.Linear(128, 1),
        nn.Sigmoid(),
    )


def train(x, y, device, epochs, seed):
    import torch

    torch.manual_seed(seed)
    model = make_model().to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=1e-3, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, epochs)
    xt = torch.from_numpy(x).to(device)
    yt = torch.from_numpy(y).to(device)
    n_pos, n_neg = float(y.sum()), float(len(y) - y.sum())
    # Les négatifs pèsent plus : un faux déclenchement coûte plus qu'un raté.
    w_pos, w_neg = 1.0, 3.0 * n_pos / n_neg
    for epoch in range(epochs):
        model.train()
        perm = torch.randperm(len(xt), device=device)
        total = 0.0
        for b in range(0, len(perm), 512):
            idx = perm[b : b + 512]
            p = model(xt[idx].float()).squeeze(1).clamp(1e-6, 1 - 1e-6)
            t = yt[idx]
            loss = -(w_pos * t * torch.log(p) + w_neg * (1 - t) * torch.log(1 - p)).mean()
            opt.zero_grad()
            loss.backward()
            opt.step()
            total += float(loss) * len(idx)
        sched.step()
        if epoch % 5 == 4 or epoch == epochs - 1:
            print(f"  époque {epoch + 1}/{epochs} : perte {total / len(xt):.4f}", flush=True)
    return model.eval().cpu()


def export_onnx(model, path: Path) -> None:
    import torch

    dummy = torch.zeros(1, FRAMES, EMB)
    torch.onnx.export(
        model, dummy, str(path), input_names=["x"], output_names=["score"],
        dynamic_axes={"x": {0: "n"}, "score": {0: "n"}}, opset_version=17, dynamo=False,
    )


# --------------------------------------------------------------- évaluation


class OnnxScorer:
    def __init__(self, path: Path, threads: int):
        import onnxruntime as ort

        opts = ort.SessionOptions()
        opts.intra_op_num_threads = threads
        self.s = ort.InferenceSession(str(path), opts, providers=["CPUExecutionProvider"])

    def __call__(self, windows: np.ndarray) -> np.ndarray:
        if len(windows) == 0:
            return np.zeros(0, np.float32)
        out = [self.s.run(None, {"x": windows[b : b + 4096]})[0][:, 0] for b in range(0, len(windows), 4096)]
        return np.concatenate(out)


def eval_clips(rows, data, feats, scorer, noisy_aug=None):
    """Score maximal de chaque clip (propre ou bruité)."""
    out = []
    for i, r in enumerate(rows):
        clip = read_wav(data / r["file"])
        if noisy_aug is None:
            audio, _ = quiet_pad(clip, 2.5, 0.5, seed=i)
        else:
            audio, _ = noisy_aug(clip, 2.5, 0.5)
        out.append(float(scorer(windows_of(feats.embeddings(audio))).max(initial=0.0)))
    return np.array(out)


def detections(scores: np.ndarray, thr: float) -> list[float]:
    """Instants (fin de fenêtre) des détections, avec anti-rebond."""
    times, last = [], -1e9
    for j, s in enumerate(scores):
        t = emb_end_time(j + FRAMES - 1)
        if s >= thr and t - last >= DEBOUNCE:
            times.append(t)
            last = t
    return times


def eval_corpus(feats, scorer, thresholds):
    corpus = kobitts.BENCH_HOME / "corpus"
    res = {t: {"hits": 0, "kobi": 0, "false": 0, "hours": 0.0} for t in thresholds}
    for d in ("fr", "en", "mixte"):
        ref = [json.loads(line) for line in open(corpus / d / "reference.jsonl", encoding="utf-8")]
        for track in ("moi", "eux"):
            audio = read_wav(corpus / d / f"{track}.wav")
            audio += np.random.default_rng(0).standard_normal(len(audio)) * 3
            scores = scorer(windows_of(feats.embeddings(audio.astype(np.int16))))
            kobi = [(r["start"], r["end"]) for r in ref if r["source"] == track and r["kobi"]]
            for t in thresholds:
                dets = detections(scores, t)
                hit = [any(s <= x <= s + 1.5 for x in dets) for s, _ in kobi]
                false = [x for x in dets if not any(s <= x <= s + 1.5 for s, _ in kobi)]
                res[t]["hits"] += sum(hit)
                res[t]["kobi"] += len(kobi)
                res[t]["false"] += len(false)
                res[t]["hours"] += len(audio) / SR / 3600
    return res


def eval_validation(scorer, thresholds):
    path = OWW_DIR / "validation_set_features.npy"
    kobitts._download(VALIDATION[0], path, VALIDATION[1])
    feats = np.load(path, mmap_mode="r")  # (n, 96), un vecteur toutes les 80 ms
    feats = np.asarray(feats, dtype=np.float32).reshape(-1, EMB)
    scores = np.concatenate([
        scorer(windows_of(feats[b : b + 200_000 + FRAMES - 1]))
        for b in range(0, len(feats) - FRAMES + 1, 200_000)
    ])
    hours = len(feats) * 0.08 / 3600
    return {t: {"false": len(detections(scores, t)), "hours": hours} for t in thresholds}, hours


# -------------------------------------------------------------------- main


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--data", type=Path, default=kobitts.BENCH_HOME / "wake")
    parser.add_argument("--out", type=Path, default=kobitts.BENCH_HOME / "models" / "kobi-oww")
    parser.add_argument("--device", default="cuda")
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--epochs", type=int, default=40)
    parser.add_argument("--aug-pos", type=int, default=3, help="passages augmentés par positif")
    parser.add_argument("--aug-neg", type=int, default=2, help="passages augmentés par négatif")
    parser.add_argument("--noise-clips", type=int, default=1500, help="clips de fond seul")
    parser.add_argument("--seed", type=int, default=1)
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    kobitts.limit_cpu(args.threads)

    import torch

    torch.set_num_threads(args.threads)
    device = args.device if args.device == "cpu" or torch.cuda.is_available() else "cpu"
    rng = random.Random(args.seed)
    feats = Features(args.threads)
    rows = load_metadata(args.data)
    train_rows = [r for r in rows if r["split"] == "train"]
    test_rows = [r for r in rows if r["split"] == "test"]
    babble = [read_wav(args.data / r["file"]) for r in train_rows if r["kind"] == "phrase"]
    aug = Augmenter(babble, args.seed)

    print(f"features : {len(train_rows)} clips d'entraînement")
    t0 = time.time()
    x, y = build_training_set(train_rows, args.data, feats, aug, args.aug_pos, args.aug_neg, args.noise_clips, rng)
    t_feat = time.time() - t0
    print(f"  {len(x)} fenêtres ({int(y.sum())} positives) en {t_feat:.0f} s")

    print(f"entraînement sur {device}")
    t0 = time.time()
    model = train(x, y, device, args.epochs, args.seed)
    t_train = time.time() - t0
    args.out.mkdir(parents=True, exist_ok=True)
    onnx_path = args.out / "kobi.onnx"
    export_onnx(model, onnx_path)
    scorer = OnnxScorer(onnx_path, args.threads)
    with torch.no_grad():
        sample = x[:256].astype(np.float32)
        ref = model(torch.from_numpy(sample)).numpy()[:, 0]
    assert np.allclose(ref, scorer(sample), atol=1e-4), "ONNX et torch divergent"

    print("évaluation")
    thresholds = [0.3, 0.5, 0.7, 0.9]
    pos = [r for r in test_rows if r["label"] == "positive"]
    neg = [r for r in test_rows if r["label"] == "negative"]
    noisy = Augmenter(babble, seed=1234)
    s_pos, s_neg = eval_clips(pos, args.data, feats, scorer), eval_clips(neg, args.data, feats, scorer)
    s_pos_n, s_neg_n = (eval_clips(pos, args.data, feats, scorer, noisy),
                        eval_clips(neg, args.data, feats, scorer, noisy))
    corpus = eval_corpus(feats, scorer, thresholds)
    validation, val_hours = eval_validation(scorer, thresholds)

    def rate(scores, t):
        return round(float((scores >= t).mean()), 4)

    by_lang = {
        lang: {t: rate(s_pos[[r["lang"] == lang for r in pos]], t) for t in thresholds}
        for lang in ("fr", "en")
    }
    metrics = {
        "thresholds": {
            str(t): {
                "test_positive_recall": rate(s_pos, t),
                "test_positive_recall_fr": by_lang["fr"][t],
                "test_positive_recall_en": by_lang["en"][t],
                "test_negative_false_accept": rate(s_neg, t),
                "test_positive_recall_noisy": rate(s_pos_n, t),
                "test_negative_false_accept_noisy": rate(s_neg_n, t),
                "corpus_kobi_detected": f"{corpus[t]['hits']}/{corpus[t]['kobi']}",
                "corpus_false_per_hour": round(corpus[t]["false"] / corpus[t]["hours"], 2),
                "validation_false_per_hour": round(validation[t]["false"] / val_hours, 2),
            }
            for t in thresholds
        },
        "test_positives": len(pos),
        "test_negatives": len(neg),
        "validation_hours": round(val_hours, 2),
        "train_windows": int(len(x)),
        "train_positive_windows": int(y.sum()),
        "feature_seconds": round(t_feat),
        "train_seconds": round(t_train),
        "device": device,
        "threads": args.threads,
    }
    manifest = json.loads((args.data / "manifest.json").read_text(encoding="utf-8"))
    card = {
        "model": "kobi.onnx",
        "architecture": "openWakeWord : melspectrogram + embedding v0.5.1, puis MLP 1536-128-128-1",
        "input": "x [n, 16, 96] (16 vecteurs d'embedding, 1,28 s de pas, ~2 s d'audio)",
        "output": "score [n, 1] entre 0 et 1",
        "feature_models": OWW_FILES,
        "data_manifest_sha256": manifest["sha256"],
        "voice_licenses": manifest.get("voice_licenses", {}),
        "train_args": {k: str(v) for k, v in vars(args).items()},
        "warning": "Prototype J0, non distribuable : voix d'entraînement dérivées de "
                   "lessac (licence de recherche) et voix Windows.",
    }
    (args.out / "metrics.json").write_text(json.dumps(metrics, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    (args.out / "card.json").write_text(json.dumps(card, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(metrics, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
