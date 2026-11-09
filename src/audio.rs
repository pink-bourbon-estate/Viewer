//! Audio preview: decode with Symphonia (in the background), play with cpal,
//! waveform, seek, loop and volume. Up to 30 minutes is decoded into memory
//! as 16-bit stereo (≈ 300 MB worst case at 48 kHz) — plenty for a preview.

use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

const MAX_SECONDS: u64 = 30 * 60;
pub const PEAKS: usize = 2000;

pub struct Decoded {
    pub rate: u32,
    pub channels_in_file: usize,
    /// Interleaved stereo i16.
    pub samples: Vec<i16>,
    pub truncated: bool,
    pub codec: String,
    pub peaks: Vec<(f32, f32)>,
}

impl Decoded {
    pub fn frames(&self) -> u64 {
        (self.samples.len() / 2) as u64
    }
    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }
}

/// Decode a whole file (runs on a worker thread).
pub fn decode(path: &Path) -> Result<Decoded, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(e) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(e);
    }
    let mut reader = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("Unsupported or damaged audio: {e}"))?;
    let track = reader
        .tracks()
        .iter()
        .find(|t| t.codec_params.as_ref().and_then(|c| c.audio()).is_some())
        .ok_or("No audio track found")?;
    let track_id = track.id;
    let params = track.codec_params.as_ref().and_then(|c| c.audio()).cloned().ok_or("No audio track found")?;
    let rate = params.sample_rate.filter(|r| *r > 0).ok_or("Unknown sample rate")?;
    let codec = format!("{}", reader.format_info().short_name);
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("Unsupported codec: {e}"))?;
    let max_frames = MAX_SECONDS * rate as u64;
    let mut samples: Vec<i16> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut channels_in_file = 0usize;
    let mut truncated = false;
    let mut errors = 0;
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(_)) | Err(SymError::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => {
                errors += 1;
                if errors > 50 {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        let ch = buf.num_planes();
        if ch == 0 {
            continue;
        }
        channels_in_file = ch;
        let n = buf.frames();
        scratch.resize(n * ch, 0.0);
        buf.copy_to_slice_interleaved(&mut scratch[..n * ch]);
        for f in scratch[..n * ch].chunks_exact(ch) {
            let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
            samples.push((l.clamp(-1.0, 1.0) * 32767.0) as i16);
            samples.push((r.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        if (samples.len() / 2) as u64 >= max_frames {
            truncated = true;
            break;
        }
    }
    if samples.is_empty() {
        return Err("The file contains no decodable audio.".into());
    }
    let frames = samples.len() / 2;
    let per = (frames / PEAKS).max(1);
    let peaks = samples
        .chunks(per * 2)
        .map(|c| {
            let (mut lo, mut hi) = (0i16, 0i16);
            for s in c {
                lo = lo.min(*s);
                hi = hi.max(*s);
            }
            (lo as f32 / 32768.0, hi as f32 / 32768.0)
        })
        .collect();
    Ok(Decoded { rate, channels_in_file, samples, truncated, codec, peaks })
}

/// Shared between the UI and the audio callback.
pub struct Transport {
    pub playing: AtomicBool,
    pub looping: AtomicBool,
    /// Position in source frames × 1024 (fixed point for resampling).
    pub pos: AtomicU64,
    /// Volume × 1000.
    pub volume: AtomicU32,
}

pub struct Player {
    pub audio: Arc<Decoded>,
    pub t: Arc<Transport>,
    _stream: Option<cpal::Stream>,
    pub device_error: Option<String>,
}

impl Player {
    pub fn new(audio: Decoded) -> Player {
        let audio = Arc::new(audio);
        let t = Arc::new(Transport {
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            volume: AtomicU32::new(800),
        });
        let (stream, device_error) = match open_stream(audio.clone(), t.clone()) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        Player { audio, t, _stream: stream, device_error }
    }

    pub fn position(&self) -> f64 {
        (self.t.pos.load(Ordering::Relaxed) >> 10) as f64 / self.audio.rate.max(1) as f64
    }

    pub fn seek(&self, seconds: f64) {
        let f = (seconds.max(0.0) * self.audio.rate as f64) as u64;
        self.t.pos.store(f.min(self.audio.frames()) << 10, Ordering::Relaxed);
    }

    pub fn toggle(&self) {
        if self.position() >= self.audio.seconds() - 0.01 {
            self.seek(0.0);
        }
        let p = self.t.playing.load(Ordering::Relaxed);
        self.t.playing.store(!p, Ordering::Relaxed);
    }

    pub fn playing(&self) -> bool {
        self.t.playing.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.t.playing.store(false, Ordering::Relaxed);
    }
}

fn open_stream(audio: Arc<Decoded>, t: Arc<Transport>) -> Result<cpal::Stream, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("No audio output device found")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let out_rate = supported.sample_rate();
    let out_ch = supported.channels() as usize;
    let fmt = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    // Source frames per output frame, in 1/1024ths.
    let step = ((audio.rate as u64) << 10) / (out_rate.max(1) as u64);
    let err_fn = |e: cpal::Error| {
        let _ = e;
    };
    macro_rules! build {
        ($t:ty) => {{
            let audio = audio.clone();
            let t = t.clone();
            device.build_output_stream(
                config,
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    fill(data, out_ch, &audio, &t, step);
                },
                err_fn,
                None,
            )
        }};
    }
    let stream = match fmt {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        other => return Err(format!("Unsupported output format {other:?}")),
    }
    .map_err(|e| format!("Could not open audio output: {e}"))?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn fill<T: cpal::Sample + cpal::FromSample<f32>>(data: &mut [T], ch: usize, a: &Decoded, t: &Transport, step: u64) {
    let silence = T::from_sample(0.0f32);
    if !t.playing.load(Ordering::Relaxed) || ch == 0 {
        data.iter_mut().for_each(|s| *s = silence);
        return;
    }
    let vol = t.volume.load(Ordering::Relaxed) as f32 / 1000.0;
    let frames = a.frames();
    let mut pos = t.pos.load(Ordering::Relaxed);
    for frame in data.chunks_mut(ch) {
        let i = pos >> 10;
        if i + 1 >= frames {
            if t.looping.load(Ordering::Relaxed) {
                pos = 0;
                continue;
            }
            t.playing.store(false, Ordering::Relaxed);
            frame.iter_mut().for_each(|s| *s = silence);
            continue;
        }
        let f = (pos & 1023) as f32 / 1024.0;
        let at = |k: u64, c: usize| a.samples.get((k as usize) * 2 + c).copied().unwrap_or(0) as f32 / 32768.0;
        let l = (at(i, 0) * (1.0 - f) + at(i + 1, 0) * f) * vol;
        let r = (at(i, 1) * (1.0 - f) + at(i + 1, 1) * f) * vol;
        for (c, s) in frame.iter_mut().enumerate() {
            *s = T::from_sample(match c {
                0 => l,
                1 => r,
                _ => (l + r) * 0.5,
            });
        }
        pos += step;
    }
    t.pos.store(pos, Ordering::Relaxed);
}

pub fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_wav() {
        // Write a 0.25 s 440 Hz mono WAV.
        let rate = 22050u32;
        let n = rate / 4;
        let mut d = Vec::new();
        let data_len = n * 2;
        d.extend_from_slice(b"RIFF");
        d.extend_from_slice(&(36 + data_len).to_le_bytes());
        d.extend_from_slice(b"WAVEfmt ");
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&rate.to_le_bytes());
        d.extend_from_slice(&(rate * 2).to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&16u16.to_le_bytes());
        d.extend_from_slice(b"data");
        d.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let v = ((i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 12000.0) as i16;
            d.extend_from_slice(&v.to_le_bytes());
        }
        let p = std::env::temp_dir().join(format!("viewer-a-{}.wav", std::process::id()));
        let _ = std::fs::write(&p, d);
        let a = decode(&p);
        let _ = std::fs::remove_file(&p);
        let a = a.unwrap_or_else(|e| Decoded { rate: 0, channels_in_file: 0, samples: vec![], truncated: false, codec: e, peaks: vec![] });
        assert_eq!(a.rate, rate, "{}", a.codec);
        assert_eq!(a.frames(), n as u64);
        assert!(a.peaks.iter().any(|p| p.1 > 0.3));
    }
}
