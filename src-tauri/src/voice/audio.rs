//! Converts a captured WAV into the 16 kHz mono `f32` samples on-device
//! engines expect. Cloud engines keep receiving the original WAV.
// Temporary: Task 4 adds the first consumer and removes this allow.
#![allow(dead_code)]

pub const LOCAL_SAMPLE_RATE: u32 = 16_000;

/// Decodes any PCM WAV and downmixes it to mono.
pub fn decode_wav(wav: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav))
        .map_err(|error| format!("could not read the recording: {error}"))?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels.max(1));
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|error| format!("could not read the recording: {error}"))?,
        hound::SampleFormat::Int => {
            let scale = (1_i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|error| format!("could not read the recording: {error}"))?
        }
    };
    let mono = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

/// Linear resampling. When downsampling, each output sample averages the
/// input span it covers, which keeps content above the new Nyquist limit
/// from folding back into the speech band.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let output_len = (input.len() as f64 / ratio).round() as usize;
    let last = input.len() - 1;
    (0..output_len)
        .map(|index| {
            let center = index as f64 * ratio;
            if ratio <= 1.0 {
                let left = (center.floor() as usize).min(last);
                let right = (left + 1).min(last);
                let fraction = (center - left as f64) as f32;
                return input[left] + (input[right] - input[left]) * fraction;
            }
            let half = ratio / 2.0;
            let start = ((center - half).ceil().max(0.0) as usize).min(last);
            let end = ((center + half).floor() as usize).min(last);
            if start > end {
                return input[(center as usize).min(last)];
            }
            let span = &input[start..=end];
            span.iter().sum::<f32>() / span.len() as f32
        })
        .collect()
}

pub fn to_local_samples(wav: &[u8]) -> Result<Vec<f32>, String> {
    let (samples, rate) = decode_wav(wav)?;
    Ok(resample(&samples, rate, LOCAL_SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frequency: f32, seconds: f32) -> Vec<f32> {
        let count = (rate as f32 * seconds) as usize;
        (0..count)
            .map(|i| (2.0 * std::f32::consts::PI * frequency * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn rising_zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count()
    }

    fn wav(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for sample in samples {
            writer
                .write_sample((sample * i16::MAX as f32) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
        cursor.into_inner()
    }

    #[test]
    fn downsampling_keeps_duration() {
        for rate in [48_000, 44_100] {
            let out = resample(&sine(rate, 440.0, 1.0), rate, LOCAL_SAMPLE_RATE);
            assert!(
                (out.len() as i64 - 16_000).abs() <= 1,
                "{rate}: {}",
                out.len()
            );
        }
    }

    #[test]
    fn downsampling_keeps_pitch() {
        for rate in [48_000, 44_100] {
            let out = resample(&sine(rate, 440.0, 1.0), rate, LOCAL_SAMPLE_RATE);
            let crossings = rising_zero_crossings(&out);
            assert!((438..=442).contains(&crossings), "{rate}: {crossings}");
        }
    }

    #[test]
    fn upsampling_keeps_duration_and_pitch() {
        let out = resample(&sine(8_000, 200.0, 1.0), 8_000, LOCAL_SAMPLE_RATE);
        assert!((out.len() as i64 - 16_000).abs() <= 1);
        assert!((198..=202).contains(&rising_zero_crossings(&out)));
    }

    #[test]
    fn same_rate_is_unchanged() {
        let input = sine(16_000, 300.0, 0.1);
        assert_eq!(resample(&input, 16_000, 16_000), input);
    }

    #[test]
    fn decodes_stereo_wav_to_mono() {
        let interleaved = [0.5, -0.5, 0.25, 0.25];
        let (mono, rate) = decode_wav(&wav(&interleaved, 22_050, 2)).unwrap();
        assert_eq!(rate, 22_050);
        assert_eq!(mono.len(), 2);
        assert!(mono[0].abs() < 0.001);
        assert!((mono[1] - 0.25).abs() < 0.001);
    }

    #[test]
    fn rejects_non_wav_bytes() {
        assert!(decode_wav(b"not a wav").is_err());
    }
}
