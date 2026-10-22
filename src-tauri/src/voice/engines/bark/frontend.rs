//! Log-mel features for Bark, a port of `Frontend` in the model's
//! `bark_coreml.py` (itself transformers' ParakeetFeatureExtractor):
//! pre-emphasis, centred 512-point STFT with a symmetric 400-sample Hann
//! window, slaney mel filters, log, and per-utterance normalization.

pub const SAMPLE_RATE: usize = 16_000;
pub const HOP: usize = 160;
pub const MELS: usize = 128;
const NFFT: usize = 512;
const WIN: usize = 400;
pub const BINS: usize = NFFT / 2 + 1;
const PREEMPHASIS: f32 = 0.97;
const LOG_GUARD: f32 = 5.960_464_5e-8; // 2^-24
const NORM_EPSILON: f32 = 1e-5;

pub struct Frontend {
    /// `[MELS][BINS]`, row-major.
    mel: Vec<f32>,
    window: [f32; NFFT],
    fft: Fft,
}

/// Normalized features, `frames × MELS` row-major, and how many frames hold
/// audio (the rest are zero).
pub struct Features {
    pub data: Vec<f32>,
    pub frames: usize,
    pub valid: usize,
}

impl Frontend {
    pub fn new(mel: Vec<f32>) -> Result<Self, String> {
        if mel.len() != MELS * BINS {
            return Err("Bark mel filters have the wrong shape".to_string());
        }
        // numpy's `hanning` is symmetric, centred in the FFT frame.
        let mut window = [0.0_f32; NFFT];
        let offset = (NFFT - WIN) / 2;
        for i in 0..WIN {
            let phase = 2.0 * std::f64::consts::PI * i as f64 / (WIN - 1) as f64;
            window[offset + i] = (0.5 - 0.5 * phase.cos()) as f32;
        }
        Ok(Self {
            mel,
            window,
            fft: Fft::new(NFFT),
        })
    }

    pub fn features(&self, samples: &[f32]) -> Features {
        let len = samples.len();
        let pad = NFFT / 2;
        let mut padded = vec![0.0_f32; len + NFFT];
        if let Some(first) = samples.first() {
            padded[pad] = *first;
        }
        for (i, pair) in samples.windows(2).enumerate() {
            padded[pad + i + 1] = pair[1] - PREEMPHASIS * pair[0];
        }
        let frames = 1 + len / HOP;
        let valid = len / HOP;
        let mut data = vec![0.0_f32; frames * MELS];
        let mut re = [0.0_f64; NFFT];
        let mut im = [0.0_f64; NFFT];
        let mut power = [0.0_f32; BINS];
        for frame in 0..frames {
            let start = frame * HOP;
            for i in 0..NFFT {
                re[i] = (padded[start + i] * self.window[i]) as f64;
                im[i] = 0.0;
            }
            self.fft.run(&mut re, &mut im);
            for bin in 0..BINS {
                power[bin] = (re[bin] * re[bin] + im[bin] * im[bin]) as f32;
            }
            let row = &mut data[frame * MELS..(frame + 1) * MELS];
            for (mel, out) in row.iter_mut().enumerate() {
                let filter = &self.mel[mel * BINS..(mel + 1) * BINS];
                let energy: f32 = filter.iter().zip(&power).map(|(f, p)| f * p).sum();
                *out = (energy + LOG_GUARD).ln();
            }
        }
        normalize(&mut data, valid);
        Features {
            data,
            frames,
            valid,
        }
    }
}

/// Per-mel mean and unbiased standard deviation over the valid frames;
/// frames past them are zeroed.
fn normalize(data: &mut [f32], valid: usize) {
    let frames = data.len() / MELS;
    let used = valid.min(frames);
    let mut mean = [0.0_f64; MELS];
    let mut deviation = [0.0_f64; MELS];
    for frame in 0..used {
        for (mel, sum) in mean.iter_mut().enumerate() {
            *sum += data[frame * MELS + mel] as f64;
        }
    }
    for sum in &mut mean {
        *sum /= used.max(1) as f64;
    }
    for frame in 0..used {
        for mel in 0..MELS {
            let delta = data[frame * MELS + mel] as f64 - mean[mel];
            deviation[mel] += delta * delta;
        }
    }
    let divisor = used.saturating_sub(1).max(1) as f64;
    for frame in 0..frames {
        for mel in 0..MELS {
            let index = frame * MELS + mel;
            data[index] = if frame < used {
                let std = (deviation[mel] / divisor).sqrt();
                ((data[index] as f64 - mean[mel]) / (std + NORM_EPSILON as f64)) as f32
            } else {
                0.0
            };
        }
    }
}

/// Iterative radix-2 complex FFT for one power-of-two size.
struct Fft {
    size: usize,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    fn new(size: usize) -> Self {
        assert!(size.is_power_of_two());
        let (cos, sin) = (0..size / 2)
            .map(|k| {
                let angle = -2.0 * std::f64::consts::PI * k as f64 / size as f64;
                (angle.cos(), angle.sin())
            })
            .unzip();
        Self { size, cos, sin }
    }

    fn run(&self, re: &mut [f64], im: &mut [f64]) {
        let n = self.size;
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let a = start + k;
                    let b = a + len / 2;
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_a_direct_dft() {
        let n = 16;
        let input: Vec<f64> = (0..n).map(|i| ((i * 7 % 5) as f64) - 1.5).collect();
        let mut re = input.clone();
        let mut im = vec![0.0; n];
        Fft::new(n).run(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0, 0.0);
            for (t, x) in input.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                sr += x * angle.cos();
                si += x * angle.sin();
            }
            assert!(
                (re[k] - sr).abs() < 1e-9 && (im[k] - si).abs() < 1e-9,
                "bin {k}"
            );
        }
    }

    #[test]
    fn window_is_numpys_symmetric_hanning_centred() {
        let frontend = Frontend::new(vec![0.0; MELS * BINS]).unwrap();
        assert_eq!(frontend.window[55], 0.0);
        assert_eq!(frontend.window[56], 0.0);
        assert!((frontend.window[56 + 199] - frontend.window[56 + 200]).abs() < 1e-7);
        assert_eq!(frontend.window[56 + 400], 0.0);
    }

    #[test]
    fn frames_follow_the_hop_and_padding_is_zeroed() {
        // One filter per mel that reads a single bin keeps the check simple.
        let mut mel = vec![0.0; MELS * BINS];
        for m in 0..MELS {
            mel[m * BINS + m + 1] = 1.0;
        }
        let frontend = Frontend::new(mel).unwrap();
        let samples: Vec<f32> = (0..4_000).map(|i| ((i as f32) * 0.1).sin() * 0.3).collect();
        let features = frontend.features(&samples);
        assert_eq!(features.frames, 1 + 4_000 / HOP);
        assert_eq!(features.valid, 4_000 / HOP);
        assert_eq!(features.data.len(), features.frames * MELS);
        let last = &features.data[features.valid * MELS..];
        assert!(last.iter().all(|value| *value == 0.0));
        // Each mel is normalized to zero mean over the valid frames.
        for m in 0..MELS {
            let mean: f32 = (0..features.valid)
                .map(|f| features.data[f * MELS + m])
                .sum::<f32>()
                / features.valid as f32;
            assert!(mean.abs() < 1e-3, "mel {m} mean {mean}");
        }
    }

    #[test]
    fn rejects_mis_shaped_filters() {
        assert!(Frontend::new(vec![0.0; 10]).is_err());
    }
}
