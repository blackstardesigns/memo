//! Microphone capture for dictation.
//!
//! Opens the default input device with `cpal`, downmixes to mono, resamples to
//! the 16 kHz f32 stream whisper.cpp expects, and pushes chunks to a channel the
//! dictation worker drains. The `cpal::Stream` is `!Send`, so it is created and
//! owned on the worker thread (see [`crate::dictation`]) and never moved across
//! threads — dropping the returned [`Capture`] stops the microphone.

use std::sync::mpsc::Sender;

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

/// Sample rate whisper.cpp expects (mono, 16 kHz, f32).
pub const TARGET_RATE: u32 = 16_000;

/// An active microphone capture. Dropping it stops and releases the stream.
pub struct Capture {
    _stream: cpal::Stream,
}

/// Open the default input device and stream mono 16 kHz f32 chunks to `tx`.
///
/// The returned [`Capture`] keeps the stream alive; drop it to stop recording.
pub fn start(tx: Sender<Vec<f32>>) -> Result<Capture> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no microphone / input device found"))?;
    let supported = device
        .default_input_config()
        .context("querying the default input configuration")?;

    let sample_format = supported.sample_format();
    let channels = supported.channels() as usize;
    let in_rate = supported.sample_rate();
    let config: StreamConfig = supported.into();

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, in_rate, tx),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, in_rate, tx),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, in_rate, tx),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, channels, in_rate, tx),
        other => bail!("unsupported microphone sample format: {other:?}"),
    }?;

    stream.play().context("starting the microphone stream")?;
    Ok(Capture { _stream: stream })
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    in_rate: u32,
    tx: Sender<Vec<f32>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut resampler = Resampler::new(in_rate, TARGET_RATE);
    let stream = device
        .build_input_stream(
            *config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono = downmix(data, channels);
                let out = resampler.process(&mono);
                if !out.is_empty() {
                    // Receiver gone (dictation stopped) just means we discard.
                    let _ = tx.send(out);
                }
            },
            // Swallow stream errors: printing here would corrupt the alt-screen
            // TUI, and a dropped stream simply ends capture.
            |_err| {},
            None,
        )
        .context("building the microphone input stream")?;
    Ok(stream)
}

/// Average interleaved channels down to a single mono track of f32 samples.
fn downmix<T>(data: &[T], channels: usize) -> Vec<f32>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    if channels <= 1 {
        return data.iter().map(|&s| f32::from_sample(s)).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
            sum / frame.len() as f32
        })
        .collect()
}

/// Streaming linear resampler from `in_rate` to `out_rate`. Keeps a fractional
/// read position and the previous buffer's last sample so interpolation is
/// continuous across the chunk boundaries cpal hands us. whisper.cpp is tolerant
/// of linear resampling, so this stays dependency-free; swap in `rubato` later if
/// transcription quality ever demands a higher-order filter.
struct Resampler {
    /// Input samples consumed per output sample (`in_rate / out_rate`).
    ratio: f64,
    /// Next output position, in input-sample units relative to the next buffer's
    /// start. May be negative (pointing back into the previous buffer's tail).
    pos: f64,
    last: f32,
    have_last: bool,
}

impl Resampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Resampler {
            ratio: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            last: 0.0,
            have_last: false,
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        // Identical rates: pass through unchanged.
        if (self.ratio - 1.0).abs() < 1e-9 {
            self.last = input[input.len() - 1];
            self.have_last = true;
            return input.to_vec();
        }

        let n = input.len();
        let last = self.last;
        let have_last = self.have_last;
        // Sample the input at fractional position `i` (relative to input[0]).
        // `i` ranges over [-1, n-1]; i in [-1,0) interpolates from the previous
        // buffer's tail (`last`) into input[0].
        let sample_at = |i: f64| -> f32 {
            if i < 0.0 {
                let frac = (i + 1.0) as f32; // [0,1)
                let a = if have_last { last } else { input[0] };
                a + (input[0] - a) * frac
            } else {
                let idx = i.floor() as usize;
                let frac = (i - idx as f64) as f32;
                let a = input[idx];
                let b = if idx + 1 < n {
                    input[idx + 1]
                } else {
                    input[idx]
                };
                a + (b - a) * frac
            }
        };

        let mut out = Vec::with_capacity((n as f64 / self.ratio) as usize + 1);
        let mut pos = self.pos;
        while pos <= (n - 1) as f64 {
            out.push(sample_at(pos));
            pos += self.ratio;
        }
        // Carry the read position into the next buffer (where input[0] sits at
        // global index n), and remember this buffer's last sample.
        self.pos = pos - n as f64;
        self.last = input[n - 1];
        self.have_last = true;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rates_match() {
        let mut r = Resampler::new(16_000, 16_000);
        let out = r.process(&[0.1, 0.2, 0.3]);
        assert_eq!(out, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn downsampling_halves_sample_count_roughly() {
        // 32 kHz -> 16 kHz across two buffers should yield ~half the samples.
        let mut r = Resampler::new(32_000, 16_000);
        let buf: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let a = r.process(&buf);
        let b = r.process(&buf);
        let total = a.len() + b.len();
        assert!((95..=105).contains(&total), "got {total} samples");
    }

    #[test]
    fn upsampling_increases_sample_count_roughly() {
        // 8 kHz -> 16 kHz roughly doubles the sample count.
        let mut r = Resampler::new(8_000, 16_000);
        let buf: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let a = r.process(&buf);
        let b = r.process(&buf);
        let total = a.len() + b.len();
        assert!((395..=405).contains(&total), "got {total} samples");
    }

    #[test]
    fn downmix_averages_stereo() {
        let interleaved = [0.0f32, 1.0, 0.5, 0.5];
        let mono = downmix(&interleaved, 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }
}
