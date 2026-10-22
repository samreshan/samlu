//! Bark's TDT decoder on the CPU, a port of `Decoder` and `segments` in the
//! model's `bark_coreml.py`: a two-layer LSTM prediction network and a joint
//! head over int8 weights with per-row scales, decoded greedily.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use super::frontend::SAMPLE_RATE;

/// One array from `decoder.npz`, widened to f32.
struct Array {
    shape: Vec<usize>,
    data: Vec<f32>,
}

/// A row-major matrix, dequantized when it was stored as int8.
struct Matrix {
    rows: usize,
    cols: usize,
    data: Vec<f32>,
}

impl Matrix {
    fn row(&self, index: usize) -> &[f32] {
        &self.data[index * self.cols..(index + 1) * self.cols]
    }

    /// `out = self · x + bias`.
    fn apply(&self, x: &[f32], bias: &[f32], out: &mut [f32]) {
        for (row, (out, bias)) in out.iter_mut().zip(bias).enumerate() {
            *out = dot(self.row(row), x) + bias;
        }
    }
}

/// Eight running sums let the compiler vectorize the loop.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0_f32; 8];
    let (a_chunks, a_tail) = a.as_chunks::<8>();
    let (b_chunks, b_tail) = b[..a.len()].as_chunks::<8>();
    for (x, y) in a_chunks.iter().zip(b_chunks) {
        for lane in 0..8 {
            lanes[lane] += x[lane] * y[lane];
        }
    }
    let tail: f32 = a_tail.iter().zip(b_tail).map(|(x, y)| x * y).sum();
    lanes.iter().sum::<f32>() + tail
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = ((bits >> 10) & 0x1f) as i32;
    let fraction = (bits & 0x3ff) as f32;
    match exponent {
        0 => sign * fraction * 2f32.powi(-24),
        0x1f if fraction == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// Reads a version 1 or 2 `.npy` holding little-endian i1, f2 or f4 data in
/// C order.
fn parse_npy(bytes: &[u8]) -> Result<Array, String> {
    let bad = || "decoder.npz holds an unreadable array".to_string();
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(bad());
    }
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 if bytes.len() >= 12 => (
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            12,
        ),
        _ => return Err(bad()),
    };
    let header = std::str::from_utf8(bytes.get(start..start + header_len).ok_or_else(bad)?)
        .map_err(|_| bad())?;
    if header.contains("'fortran_order': True") {
        return Err(bad());
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|rest| rest.split('\'').nth(1))
        .ok_or_else(bad)?;
    let shape: Vec<usize> = header
        .split("'shape':")
        .nth(1)
        .and_then(|rest| rest.split('(').nth(1))
        .and_then(|rest| rest.split(')').next())
        .ok_or_else(bad)?
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().map_err(|_| bad()))
        .collect::<Result<_, _>>()?;
    let count: usize = shape.iter().product();
    let body = &bytes[start + header_len..];
    let data: Vec<f32> = match descr {
        "|i1" if body.len() >= count => body[..count].iter().map(|b| *b as i8 as f32).collect(),
        "<f2" if body.len() >= count * 2 => body[..count * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f16_to_f32(u16::from_le_bytes(*pair)))
            .collect(),
        "<f4" if body.len() >= count * 4 => body[..count * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect(),
        _ => return Err(bad()),
    };
    Ok(Array { shape, data })
}

fn read_npz(path: &Path) -> Result<HashMap<String, Array>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not open Bark's decoder: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Bark's decoder is unreadable: {error}"))?;
    let mut arrays = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Bark's decoder is unreadable: {error}"))?;
        let Some(name) = entry.name().strip_suffix(".npy").map(str::to_string) else {
            continue;
        };
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Bark's decoder is unreadable: {error}"))?;
        arrays.insert(name, parse_npy(&bytes)?);
    }
    Ok(arrays)
}

struct Weights(HashMap<String, Array>);

impl Weights {
    fn vector(&mut self, name: &str) -> Result<Vec<f32>, String> {
        self.0
            .remove(name)
            .map(|array| array.data)
            .ok_or_else(|| format!("Bark's decoder is missing {name}"))
    }

    /// `name.q` int8 values times the `name.s` per-row scale, or `name` as is.
    fn matrix(&mut self, name: &str) -> Result<Matrix, String> {
        let missing = || format!("Bark's decoder is missing {name}");
        let (array, scales) = match self.0.remove(&format!("{name}.q")) {
            Some(array) => (array, Some(self.vector(&format!("{name}.s"))?)),
            None => (self.0.remove(name).ok_or_else(missing)?, None),
        };
        let [rows, cols] = array.shape[..] else {
            return Err(format!("Bark's decoder has a mis-shaped {name}"));
        };
        let mut data = array.data;
        if let Some(scales) = scales {
            if scales.len() != rows {
                return Err(format!("Bark's decoder has mis-shaped scales for {name}"));
            }
            for (row, scale) in data.chunks_exact_mut(cols).zip(scales) {
                row.iter_mut().for_each(|value| *value *= scale);
            }
        }
        Ok(Matrix { rows, cols, data })
    }
}

#[derive(Deserialize)]
struct Vocab {
    pieces: Vec<String>,
    special_ids: Vec<usize>,
    blank_id: usize,
    vocab_size: usize,
    durations: Vec<usize>,
    max_symbols_per_step: usize,
}

struct Layer {
    input: Matrix,
    hidden: Matrix,
    bias: Vec<f32>,
}

pub struct Decoder {
    embedding: Matrix,
    layers: Vec<Layer>,
    projector: Matrix,
    projector_bias: Vec<f32>,
    head: Matrix,
    head_bias: Vec<f32>,
    vocab: Vocab,
    special: HashSet<usize>,
    pub mel_filters: Vec<f32>,
}

/// The prediction network's LSTM state, one `(h, c)` pair per layer.
type State = Vec<(Vec<f32>, Vec<f32>)>;

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

impl Decoder {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let vocab: Vocab = serde_json::from_slice(
            &std::fs::read(dir.join("vocab.json"))
                .map_err(|error| format!("Could not read Bark's vocabulary: {error}"))?,
        )
        .map_err(|error| format!("Bark's vocabulary is unreadable: {error}"))?;
        let mut weights = Weights(read_npz(&dir.join("decoder.npz"))?);
        let embedding = weights.matrix("decoder.embedding.weight")?;
        let mut layers = Vec::new();
        for layer in 0..2 {
            let mut bias = weights.vector(&format!("decoder.lstm.bias_ih_l{layer}"))?;
            let hidden_bias = weights.vector(&format!("decoder.lstm.bias_hh_l{layer}"))?;
            bias.iter_mut().zip(hidden_bias).for_each(|(a, b)| *a += b);
            layers.push(Layer {
                input: weights.matrix(&format!("decoder.lstm.weight_ih_l{layer}"))?,
                hidden: weights.matrix(&format!("decoder.lstm.weight_hh_l{layer}"))?,
                bias,
            });
        }
        let projector = weights.matrix("decoder.decoder_projector.weight")?;
        let projector_bias = weights.vector("decoder.decoder_projector.bias")?;
        let head = weights.matrix("joint.head.weight")?;
        let head_bias = weights.vector("joint.head.bias")?;
        let mel_filters = weights.vector("mel_filters")?;
        let width = embedding.cols;
        let consistent = layers.iter().all(|layer| {
            layer.input.rows == 4 * width
                && layer.hidden.cols == width
                && layer.bias.len() == 4 * width
        }) && projector.cols == width
            && projector_bias.len() == projector.rows
            && head.cols == projector.rows
            && head.rows == vocab.vocab_size + vocab.durations.len()
            && head_bias.len() == head.rows
            && vocab.blank_id < vocab.vocab_size
            && embedding.rows > vocab.blank_id;
        if !consistent {
            return Err("Bark's decoder does not match its vocabulary".to_string());
        }
        let special = vocab.special_ids.iter().copied().collect();
        Ok(Self {
            embedding,
            layers,
            projector,
            projector_bias,
            head,
            head_bias,
            vocab,
            special,
            mel_filters,
        })
    }

    /// The width of the encoder output the joint head expects.
    pub fn encoder_width(&self) -> usize {
        self.head.cols
    }

    fn step(&self, token: usize, state: &mut State) -> Vec<f32> {
        let mut x = self.embedding.row(token).to_vec();
        let width = x.len();
        let mut gates = vec![0.0; 4 * width];
        for (layer, (h, c)) in self.layers.iter().zip(state.iter_mut()) {
            for (row, gate) in gates.iter_mut().enumerate() {
                *gate =
                    dot(layer.input.row(row), &x) + dot(layer.hidden.row(row), h) + layer.bias[row];
            }
            // PyTorch's gate order: input, forget, cell, output.
            for unit in 0..width {
                let i = sigmoid(gates[unit]);
                let f = sigmoid(gates[width + unit]);
                let g = gates[2 * width + unit].tanh();
                let o = sigmoid(gates[3 * width + unit]);
                c[unit] = f * c[unit] + i * g;
                h[unit] = o * c[unit].tanh();
            }
            x.clone_from(h);
        }
        let mut out = vec![0.0; self.projector.rows];
        self.projector.apply(&x, &self.projector_bias, &mut out);
        out
    }

    /// Greedy TDT decoding of `frames × encoder_width` encoder output.
    pub fn decode(&self, encoded: &[f32], frames: usize) -> Vec<usize> {
        let width = self.encoder_width();
        let hidden = self.embedding.cols;
        let mut state: State = (0..self.layers.len())
            .map(|_| (vec![0.0; hidden], vec![0.0; hidden]))
            .collect();
        let mut prediction = self.step(self.vocab.blank_id, &mut state);
        let mut joint = vec![0.0; width];
        let mut logits = vec![0.0; self.head.rows];
        let (mut t, mut emitted, mut out) = (0, 0, Vec::new());
        while t < frames {
            let frame = &encoded[t * width..(t + 1) * width];
            for ((j, e), p) in joint.iter_mut().zip(frame).zip(&prediction) {
                *j = (e + p).max(0.0);
            }
            self.head.apply(&joint, &self.head_bias, &mut logits);
            let size = self.vocab.vocab_size;
            let token = argmax(&logits[..size]);
            let mut duration = self.vocab.durations[argmax(&logits[size..])];
            if token == self.vocab.blank_id {
                t += duration.max(1);
                emitted = 0;
            } else {
                out.push(token);
                prediction = self.step(token, &mut state);
                emitted += 1;
                if duration == 0 && emitted >= self.vocab.max_symbols_per_step {
                    duration = 1;
                }
                if duration > 0 {
                    emitted = 0;
                }
                t += duration;
            }
        }
        out
    }

    pub fn detokenize(&self, ids: &[usize]) -> String {
        ids.iter()
            .filter(|id| !self.special.contains(id))
            .filter_map(|id| self.vocab.pieces.get(*id))
            .map(String::as_str)
            .collect::<String>()
            .replace('\u{2581}', " ")
            .trim()
            .to_string()
    }
}

/// Encoder frames for `valid` feature frames: three stride-2 convolutions.
pub fn encoded_frames(valid: usize) -> usize {
    (0..3).fold(valid, |n, _| n.div_ceil(2))
}

/// Cuts audio longer than `max_seconds` at pauses: within each stretch from
/// `min_seconds` to `max_seconds`, it cuts in the middle of the quietest
/// 200 ms (smoothed 10 ms energy), so no word is split.
pub fn segments(samples: &[f32], max_seconds: f32, min_seconds: f32) -> Vec<&[f32]> {
    let max = (max_seconds * SAMPLE_RATE as f32) as usize;
    let min = (min_seconds * SAMPLE_RATE as f32) as usize;
    if samples.len() <= max {
        return vec![samples];
    }
    let frame = SAMPLE_RATE / 100;
    let energy: Vec<f32> = samples[..samples.len() / frame * frame]
        .chunks_exact(frame)
        .map(|chunk| chunk.iter().map(|s| s * s).sum::<f32>() / frame as f32)
        .collect();
    // numpy's `convolve(..., ones(20) / 20, mode="same")`: frames i-10..=i+9.
    let smoothed: Vec<f32> = (0..energy.len())
        .map(|i| {
            let from = i.saturating_sub(10);
            let to = (i + 10).min(energy.len());
            energy[from..to].iter().sum::<f32>() / 20.0
        })
        .collect();
    let mut out = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let lo = (start + min) / frame;
        let hi = ((start + max) / frame).min(smoothed.len()).max(lo + 1);
        let quietest = lo + argmin(&smoothed[lo..hi]);
        let cut = quietest * frame + frame / 2;
        out.push(&samples[start..cut]);
        start = cut;
    }
    out.push(&samples[start..]);
    out
}

fn argmin(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value < values[best] {
            best = index;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npy(descr: &str, shape: &str, body: &[u8]) -> Vec<u8> {
        let mut header =
            format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
        bytes.extend((header.len() as u16).to_le_bytes());
        bytes.extend(header.as_bytes());
        bytes.extend(body);
        bytes
    }

    #[test]
    fn half_floats_convert_exactly() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!(f16_to_f32(0x7c00).is_infinite());
    }

    #[test]
    fn npy_reads_int8_half_and_float_arrays() {
        let int8 = parse_npy(&npy("|i1", "(2, 3)", &[1, 255, 3, 128, 0, 127])).unwrap();
        assert_eq!(int8.shape, vec![2, 3]);
        assert_eq!(int8.data, vec![1.0, -1.0, 3.0, -128.0, 0.0, 127.0]);
        let half = parse_npy(&npy("<f2", "(2,)", &[0x00, 0x3c, 0x00, 0xc0])).unwrap();
        assert_eq!(half.shape, vec![2]);
        assert_eq!(half.data, vec![1.0, -2.0]);
        let float = parse_npy(&npy("<f4", "(1,)", &0.5_f32.to_le_bytes())).unwrap();
        assert_eq!(float.data, vec![0.5]);
    }

    #[test]
    fn npy_rejects_other_files_and_short_bodies() {
        assert!(parse_npy(b"PK\x03\x04 not numpy").is_err());
        assert!(parse_npy(&npy("<f4", "(4,)", &[0; 8])).is_err());
        assert!(parse_npy(&npy("<i8", "(1,)", &[0; 8])).is_err());
    }

    #[test]
    fn quantized_matrices_scale_each_row() {
        let mut arrays = HashMap::new();
        arrays.insert(
            "w.q".to_string(),
            parse_npy(&npy("|i1", "(2, 2)", &[1, 2, 3, 4])).unwrap(),
        );
        arrays.insert(
            "w.s".to_string(),
            parse_npy(&npy("<f2", "(2,)", &[0x00, 0x38, 0x00, 0x40])).unwrap(),
        );
        let matrix = Weights(arrays).matrix("w").unwrap();
        assert_eq!(matrix.data, vec![0.5, 1.0, 6.0, 8.0]);
        let mut out = [0.0; 2];
        matrix.apply(&[1.0, 1.0], &[0.0, 1.0], &mut out);
        assert_eq!(out, [1.5, 15.0]);
    }

    #[test]
    fn dot_handles_lengths_off_the_lane_width() {
        let a: Vec<f32> = (0..13).map(|i| i as f32).collect();
        let b = vec![2.0; 13];
        assert_eq!(dot(&a, &b), 156.0);
    }

    #[test]
    fn encoder_frames_follow_three_halvings() {
        assert_eq!(encoded_frames(1500), 188);
        assert_eq!(encoded_frames(800), 100);
        assert_eq!(encoded_frames(1), 1);
        assert_eq!(encoded_frames(0), 0);
    }

    #[test]
    fn short_audio_is_one_segment() {
        let audio = vec![0.1; SAMPLE_RATE * 10];
        assert_eq!(segments(&audio, 14.75, 5.0).len(), 1);
    }

    #[test]
    fn long_audio_is_cut_at_the_quiet_part() {
        // Loud everywhere except one quiet second centred at 9.5 s.
        let mut audio = vec![0.5_f32; SAMPLE_RATE * 20];
        audio[SAMPLE_RATE * 9..SAMPLE_RATE * 10].fill(0.0);
        let pieces = segments(&audio, 14.75, 5.0);
        assert_eq!(pieces.len(), 2);
        let cut = pieces[0].len() as f32 / SAMPLE_RATE as f32;
        assert!((9.0..10.0).contains(&cut), "cut at {cut} s");
        assert_eq!(pieces.iter().map(|p| p.len()).sum::<usize>(), audio.len());
        assert!(pieces
            .iter()
            .all(|p| p.len() <= (14.75 * SAMPLE_RATE as f32) as usize));
    }
}
