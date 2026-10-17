"""Tests rapides de l'outillage du mot-clé : numpy seul, ni modèle ni synthèse.

python -m unittest discover -s bench/wake -p "test_*.py"
"""

import importlib.util
import sys
import unittest
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "tts"))
import kobitts  # noqa: E402


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


train = load("train_oww", HERE / "train_oww.py")
gen = load("wake_generate", HERE / "generate.py")


class Features(unittest.TestCase):
    def test_last_embedding_ends_within_the_last_80_ms(self):
        for n_samples in (16_000, 32_000, 47_999):
            mel_frames = n_samples // 160 - 3  # sortie de melspectrogram.onnx
            n_emb = (mel_frames - 76) // 8 + 1
            end = train.emb_end_time(n_emb - 1)
            self.assertLessEqual(end, n_samples / train.SR)
            self.assertGreater(end, n_samples / train.SR - 0.08)

    def test_model_needs_about_two_seconds(self):
        self.assertAlmostEqual(train.emb_end_time(train.FRAMES - 1), 1.99, places=2)

    def test_windows_of_slides_one_vector_at_a_time(self):
        emb = np.arange(20 * 96, dtype=np.float32).reshape(20, 96)
        w = train.windows_of(emb)
        self.assertEqual(w.shape, (5, 16, 96))
        np.testing.assert_array_equal(w[2], emb[2:18])
        self.assertEqual(train.windows_of(emb[:10]).shape, (0, 16, 96))


class Augmentation(unittest.TestCase):
    def test_colored_noise_is_normalised_and_tilted(self):
        rng = np.random.default_rng(0)
        for kind in ("white", "pink", "brown"):
            x = train.colored_noise(32_000, kind, rng)
            self.assertAlmostEqual(float(x.std()), 1.0, places=1)
        spec = np.abs(np.fft.rfft(train.colored_noise(32_000, "brown", rng))) ** 2
        self.assertGreater(spec[10:100].mean(), 100 * spec[-1000:].mean())

    def test_rir_has_unit_energy_and_direct_path(self):
        rir = train.synthetic_rir(np.random.default_rng(1))
        self.assertAlmostEqual(float(np.sum(rir**2)), 1.0, places=6)
        self.assertEqual(int(np.argmax(np.abs(rir))), 0)

    def test_augmenter_places_the_clip(self):
        babble = [np.ones(8000, np.float32) * 1000]
        aug = train.Augmenter(babble, seed=3)
        clip = np.sin(np.arange(4000) / 10).astype(np.float32) * 5000
        for _ in range(20):
            audio, start = aug(clip, pre=2.0, post=0.4)
            self.assertEqual(audio.dtype, np.int16)
            self.assertEqual(start, 32_000)
            self.assertGreaterEqual(len(audio), start + len(clip) + 6400)


class Detections(unittest.TestCase):
    def test_debounce(self):
        scores = np.zeros(100, np.float32)
        scores[[10, 11, 12, 40]] = 0.9  # 10 à 12 : une seule détection
        times = train.detections(scores, 0.5)
        self.assertEqual(len(times), 2)
        self.assertAlmostEqual(times[1] - times[0], 30 * 0.08, places=6)


class Generator(unittest.TestCase):
    def clip(self, lang, i):
        return gen.Clip(id=str(i), label="positive", kind="wake", lang=lang, text="Kobi.",
                        voice="v", speaker=None, speaker_name="v", length_scale=1.0, pitch=1.0)

    def test_outliers_are_relative_to_the_language_median(self):
        clips = [self.clip("fr", i) for i in range(5)] + [self.clip("en", i) for i in range(5)]
        ratios = [0.06, 0.07, 0.065, 0.3, 0.06] + [0.2, 0.21, 0.19, 0.2, 0.22]
        self.assertEqual(gen.outliers(clips, ratios), [False] * 3 + [True] + [False] * 6)

    def test_noise_levels_include_a_deterministic_one(self):
        self.assertIn((0.0, 0.0), gen.NOISES)

    def test_test_voices_are_never_trained_on(self):
        for lang in gen.LANGS:
            for voice in gen.VOICES[lang]:
                if voice in gen.TEST_VOICES:
                    self.assertNotIn("propre", kobitts.VOICE_LICENSES[voice])
        self.assertIn("fr_FR-tom-medium", gen.TEST_VOICES)  # AGPL : test seulement


class Tts(unittest.TestCase):
    def test_spoken_respells_kobi_in_english_only(self):
        self.assertEqual(kobitts.spoken("Hey Kobi, Kobis.", "en"), "Hey Koby, Kobis.")
        self.assertEqual(kobitts.spoken("Hé Kobi.", "fr"), "Hé Kobi.")

    def test_pitch_shift_changes_duration(self):
        pcm = (np.sin(np.arange(16_000) / 5) * 8000).astype("<i2").tobytes()
        up = kobitts.pitch_shift(pcm, 1.1)
        self.assertAlmostEqual(kobitts.duration(up), 1 / 1.1, places=2)
        self.assertEqual(kobitts.pitch_shift(pcm, 1.0), pcm)

    def test_every_voice_has_a_license(self):
        for lang in gen.LANGS:
            for voice in gen.VOICES[lang]:
                self.assertIn(voice, kobitts.VOICE_LICENSES)


if __name__ == "__main__":
    unittest.main()
