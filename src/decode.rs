//! Audio files (wav, mp3, flac, ogg) to the 16-bit PCM the ADX encoder takes.
//! Pure: no game calls. Anything that fails returns an error and the game's
//! own sound is used instead.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Highest sample rate handed to the game (its voice pools are made for 48 kHz).
pub const MAX_RATE: u32 = 48000;
/// Longest accepted sound (memory: ~0.56 byte per sample and channel as ADX).
pub const MAX_SECONDS: usize = 15 * 60;

#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    /// Interleaved samples.
    pub samples: Vec<i16>,
    pub channels: usize,
    pub rate: u32,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
    pub fn millis(&self) -> i64 {
        (self.frames() as i64 * 1000) / self.rate.max(1) as i64
    }
}

/// Decodes a file into interleaved f32 samples.
pub fn decode_file(path: &Path) -> Result<(Vec<f32>, usize, u32), String> {
    let file = File::open(path).map_err(|e| format!("open: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("unknown format: {e}"))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or("no audio track")?
        .clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("unsupported codec: {e}"))?;

    let mut out: Vec<f32> = Vec::new();
    let mut channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(0);
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("read: {e}")),
        };
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A damaged packet: skip it, like players do.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode: {e}")),
        };
        let spec = *decoded.spec();
        channels = spec.channels.count();
        rate = spec.rate;
        let b = buf.get_or_insert_with(|| SampleBuffer::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * channels {
            *b = SampleBuffer::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        out.extend_from_slice(b.samples());
        if channels > 0 && rate > 0 && out.len() / channels > MAX_SECONDS * rate as usize {
            return Err(format!("longer than {} minutes", MAX_SECONDS / 60));
        }
    }
    if channels == 0 || rate == 0 {
        return Err("no channel / sample rate information".into());
    }
    if out.is_empty() {
        return Err("no audio decoded".into());
    }
    Ok((out, channels, rate))
}

/// Mono stays mono, everything else becomes stereo (first two channels, the rest
/// mixed in); rates above `MAX_RATE` are resampled down; then 16-bit.
pub fn prepare(samples: &[f32], channels: usize, rate: u32) -> Pcm {
    let frames = samples.len() / channels.max(1);
    let out_ch = if channels == 1 { 1 } else { 2 };
    let mut mixed = Vec::with_capacity(frames * out_ch);
    for f in 0..frames {
        let frame = &samples[f * channels..(f + 1) * channels];
        if out_ch == 1 {
            mixed.push(frame[0]);
        } else {
            let (mut l, mut r) = (frame[0], frame[1]);
            for (i, &x) in frame.iter().enumerate().skip(2) {
                // Extra channels (centre, surrounds, LFE) split over both sides.
                if i == 3 { continue; } // LFE in the usual 5.1/7.1 order
                l += x * 0.5;
                r += x * 0.5;
            }
            mixed.push(l);
            mixed.push(r);
        }
    }
    let (mixed, rate) = if rate > MAX_RATE { (resample(&mixed, out_ch, rate, MAX_RATE), MAX_RATE) } else { (mixed, rate) };
    let samples = mixed.iter().map(|&x| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect();
    Pcm { samples, channels: out_ch, rate }
}

/// Linear-interpolation resampler (only ever used to go down to 48 kHz).
pub fn resample(samples: &[f32], channels: usize, from: u32, to: u32) -> Vec<f32> {
    let frames = samples.len() / channels;
    if frames == 0 || from == to {
        return samples.to_vec();
    }
    let out_frames = ((frames as u64 * to as u64) / from as u64) as usize;
    let step = from as f64 / to as f64;
    let mut out = Vec::with_capacity(out_frames * channels);
    for i in 0..out_frames {
        let pos = i as f64 * step;
        let a = (pos as usize).min(frames - 1);
        let b = (a + 1).min(frames - 1);
        let t = (pos - a as f64) as f32;
        for ch in 0..channels {
            let x = samples[a * channels + ch];
            let y = samples[b * channels + ch];
            out.push(x + (y - x) * t);
        }
    }
    out
}

pub fn load(path: &Path) -> Result<Pcm, String> {
    let (samples, channels, rate) = decode_file(path)?;
    Ok(prepare(&samples, channels, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(path: &Path, channels: u16, rate: u32, frames: usize) {
        let mut d = Vec::new();
        let data_len = frames * channels as usize * 2;
        d.extend_from_slice(b"RIFF");
        d.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        d.extend_from_slice(b"WAVEfmt ");
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&channels.to_le_bytes());
        d.extend_from_slice(&rate.to_le_bytes());
        d.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        d.extend_from_slice(&(channels * 2).to_le_bytes());
        d.extend_from_slice(&16u16.to_le_bytes());
        d.extend_from_slice(b"data");
        d.extend_from_slice(&(data_len as u32).to_le_bytes());
        for i in 0..frames {
            for ch in 0..channels {
                let v = ((i as f64 * 0.05 * (ch + 1) as f64).sin() * 10000.0) as i16;
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write(path, d).unwrap();
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("uma_sound_replace_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn decodes_wav() {
        let p = tmp("a.wav");
        wav(&p, 2, 44100, 4410);
        let pcm = load(&p).unwrap();
        assert_eq!((pcm.channels, pcm.rate, pcm.frames()), (2, 44100, 4410));
        assert_eq!(pcm.millis(), 100);
        let expect = ((1.0f64 * 0.05).sin() * 10000.0) as i16;
        assert!((pcm.samples[2] - expect).abs() <= 1);
    }

    #[test]
    fn high_rate_is_resampled() {
        let p = tmp("b.wav");
        wav(&p, 1, 96000, 9600);
        let pcm = load(&p).unwrap();
        assert_eq!((pcm.channels, pcm.rate, pcm.frames()), (1, 48000, 4800));
    }

    #[test]
    fn broken_files_are_errors() {
        let p = tmp("c.mp3");
        std::fs::write(&p, b"this is not audio at all").unwrap();
        assert!(load(&p).is_err());
        let p = tmp("d.wav");
        std::fs::write(&p, b"RIFF\x10\0\0\0WAVEjunk").unwrap();
        assert!(load(&p).is_err());
        assert!(load(&tmp("missing.flac")).is_err());
    }

    #[test]
    fn surround_folds_to_stereo() {
        let s = vec![0.1, 0.2, 0.4, 0.9, 0.0, 0.0];
        let pcm = prepare(&s, 6, 48000);
        assert_eq!(pcm.channels, 2);
        assert_eq!(pcm.samples, vec![(0.3f32 * 32767.0).round() as i16, (0.4f32 * 32767.0).round() as i16]);
    }
}
