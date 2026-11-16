//! CRI ADX encoder (type 3, 4-bit ADPCM, 18-byte frames of 32 samples).
//!
//! The game's audio library (CRI ADX2) decodes ADX natively, so a replacement is
//! turned into ADX once and handed to the game's own player: volume, pause and
//! stop then work as for the game's sounds. Pure and tested (with a decoder that
//! follows vgmstream's `decode_adx`).

pub const SAMPLES_PER_FRAME: usize = 32;
pub const FRAME_BYTES: usize = 18;
const HIGHPASS: u16 = 500;
/// Header (version 3 incl. its loop section) ends at 0x2C; "(c)CRI" ends at DATA.
const COPYRIGHT_OFFSET: u16 = 0x30;
pub const DATA: usize = COPYRIGHT_OFFSET as usize + 4;
/// Largest scale a frame header can hold (the top bits mark the end frame).
const MAX_SCALE: i32 = 0x1FFF;

/// Prediction coefficients for a sample rate (4.12 fixed point).
pub fn coefs(rate: u32) -> (i32, i32) {
    let z = (2.0 * std::f64::consts::PI * HIGHPASS as f64 / rate as f64).cos();
    let a = std::f64::consts::SQRT_2 - z;
    let b = std::f64::consts::SQRT_2 - 1.0;
    let c = (a - ((a + b) * (a - b)).sqrt()) / b;
    ((c * 8192.0) as i32, (c * c * -4096.0) as i32)
}

fn predict(c: (i32, i32), h1: i32, h2: i32) -> i32 {
    (c.0 * h1 + c.1 * h2) >> 12
}

/// One frame of one channel. `hist` = last two decoded samples, updated.
fn encode_frame(pcm: &[i16; SAMPLES_PER_FRAME], c: (i32, i32), hist: &mut (i32, i32), out: &mut [u8]) {
    // Open-loop estimate of the residual range, then closed-loop quantization;
    // a clipped nibble grows the scale and retries.
    let mut peak = 0;
    let (mut h1, mut h2) = *hist;
    for &s in pcm {
        peak = peak.max((s as i32 - predict(c, h1, h2)).abs());
        h2 = h1;
        h1 = s as i32;
    }
    let mut scale = ((peak + 6) / 7).clamp(1, MAX_SCALE);
    loop {
        let (mut h1, mut h2) = *hist;
        let mut nibbles = [0u8; SAMPLES_PER_FRAME];
        let mut clipped = false;
        for (i, &s) in pcm.iter().enumerate() {
            let p = predict(c, h1, h2);
            let want = s as i32 - p;
            let q = (want as f64 / scale as f64).round() as i32;
            if !(-8..=7).contains(&q) {
                clipped = true;
            }
            let q = q.clamp(-8, 7);
            let dec = (q * scale + p).clamp(i16::MIN as i32, i16::MAX as i32);
            nibbles[i] = (q & 0xF) as u8;
            h2 = h1;
            h1 = dec;
        }
        if clipped && scale < MAX_SCALE {
            scale = (scale + scale / 8 + 1).min(MAX_SCALE);
            continue;
        }
        out[0..2].copy_from_slice(&(scale as u16).to_be_bytes());
        for i in 0..16 {
            out[2 + i] = (nibbles[2 * i] << 4) | nibbles[2 * i + 1];
        }
        *hist = (h1, h2);
        return;
    }
}

/// Encodes interleaved 16-bit PCM. `looped` = the whole file loops (BGM).
pub fn encode(pcm: &[i16], channels: usize, rate: u32, looped: bool) -> Vec<u8> {
    assert!((1..=8).contains(&channels));
    let samples = pcm.len() / channels;
    let frames = samples.div_ceil(SAMPLES_PER_FRAME);
    let data_len = frames * FRAME_BYTES * channels;
    let mut out = vec![0u8; DATA + data_len + FRAME_BYTES];

    out[0..2].copy_from_slice(&0x8000u16.to_be_bytes());
    out[2..4].copy_from_slice(&COPYRIGHT_OFFSET.to_be_bytes());
    out[4] = 3; // encoding: standard ADX
    out[5] = FRAME_BYTES as u8;
    out[6] = 4; // bits per sample
    out[7] = channels as u8;
    out[8..12].copy_from_slice(&rate.to_be_bytes());
    out[12..16].copy_from_slice(&(samples as u32).to_be_bytes());
    out[16..18].copy_from_slice(&HIGHPASS.to_be_bytes());
    out[18] = 3; // version
    out[19] = 0; // flags (no encryption)
    if looped && samples > 0 {
        let loop_flag = 1u32;
        out[0x16..0x18].copy_from_slice(&1u16.to_be_bytes());
        out[0x18..0x1C].copy_from_slice(&loop_flag.to_be_bytes());
        out[0x1C..0x20].copy_from_slice(&0u32.to_be_bytes());
        out[0x20..0x24].copy_from_slice(&(DATA as u32).to_be_bytes());
        out[0x24..0x28].copy_from_slice(&(samples as u32).to_be_bytes());
        out[0x28..0x2C].copy_from_slice(&((DATA + data_len) as u32).to_be_bytes());
    }
    out[DATA - 6..DATA].copy_from_slice(b"(c)CRI");

    let c = coefs(rate);
    let mut hist = vec![(0i32, 0i32); channels];
    let mut block = [0i16; SAMPLES_PER_FRAME];
    for f in 0..frames {
        for ch in 0..channels {
            for (i, b) in block.iter_mut().enumerate() {
                let s = f * SAMPLES_PER_FRAME + i;
                *b = if s < samples { pcm[s * channels + ch] } else { 0 };
            }
            let at = DATA + (f * channels + ch) * FRAME_BYTES;
            encode_frame(&block, c, &mut hist[ch], &mut out[at..at + FRAME_BYTES]);
        }
    }
    // End frame: 0x8001, bytes left after these 4, padding.
    let end = DATA + data_len;
    out[end..end + 2].copy_from_slice(&0x8001u16.to_be_bytes());
    out[end + 2..end + 4].copy_from_slice(&((FRAME_BYTES - 4) as u16).to_be_bytes());
    out
}

/// Header fields: (channels, rate, samples, looped). For tests and checks.
#[allow(dead_code)]
pub fn header(adx: &[u8]) -> Option<(usize, u32, usize, bool)> {
    if adx.len() < DATA || adx[0..2] != [0x80, 0x00] || adx[4] != 3 {
        return None;
    }
    let be32 = |o: usize| u32::from_be_bytes(adx[o..o + 4].try_into().unwrap());
    Some((adx[7] as usize, be32(8), be32(12) as usize, be32(0x18) != 0))
}

/// Reference decoder (vgmstream's), used by the tests.
#[cfg(test)]
pub fn decode(adx: &[u8]) -> Option<Vec<i16>> {
    let (channels, rate, samples, _) = header(adx)?;
    let start = u16::from_be_bytes([adx[2], adx[3]]) as usize + 4;
    let c = coefs(rate);
    let frames = samples.div_ceil(SAMPLES_PER_FRAME);
    let mut hist = vec![(0i32, 0i32); channels];
    let mut out = vec![0i16; frames * SAMPLES_PER_FRAME * channels];
    for f in 0..frames {
        for ch in 0..channels {
            let at = start + (f * channels + ch) * FRAME_BYTES;
            let frame = adx.get(at..at + FRAME_BYTES)?;
            let scale = (u16::from_be_bytes([frame[0], frame[1]]) & 0x1FFF) as i32;
            for i in 0..SAMPLES_PER_FRAME {
                let byte = frame[2 + i / 2];
                let n = if i % 2 == 0 { byte >> 4 } else { byte & 0xF };
                let n = ((n as i8) << 4 >> 4) as i32;
                let (h1, h2) = hist[ch];
                let s = (n * scale + predict(c, h1, h2)).clamp(i16::MIN as i32, i16::MAX as i32);
                hist[ch] = (s, h1);
                out[(f * SAMPLES_PER_FRAME + i) * channels + ch] = s as i16;
            }
        }
    }
    out.truncate(samples * channels);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snr_db(a: &[i16], b: &[i16]) -> f64 {
        let sig: f64 = a.iter().map(|&x| (x as f64).powi(2)).sum();
        let err: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum();
        10.0 * (sig / err.max(1e-9)).log10()
    }

    fn tone(rate: u32, secs: f64, channels: usize) -> Vec<i16> {
        let n = (rate as f64 * secs) as usize;
        let mut v = Vec::with_capacity(n * channels);
        for i in 0..n {
            let t = i as f64 / rate as f64;
            for ch in 0..channels {
                let f = 440.0 * (ch + 1) as f64;
                let x = 0.6 * (2.0 * std::f64::consts::PI * f * t).sin()
                    + 0.2 * (2.0 * std::f64::consts::PI * 3.1 * f * t).sin();
                v.push((x * 20000.0) as i16);
            }
        }
        v
    }

    #[test]
    fn coefficients_are_a_stable_predictor() {
        for rate in [22050, 32000, 44100, 48000] {
            let (c1, c2) = coefs(rate);
            // Poles inside the unit circle: |c2| < 4096 and |c1| < 4096 - c2.
            assert!(c2 < 0 && -c2 < 4096 && c1 > 0 && c1 < 4096 - c2, "{rate}: {c1} {c2}");
        }
    }

    #[test]
    fn header_fields() {
        let pcm = tone(48000, 0.5, 2);
        let adx = encode(&pcm, 2, 48000, true);
        assert_eq!(header(&adx), Some((2, 48000, 24000, true)));
        assert_eq!(&adx[DATA - 6..DATA], b"(c)CRI");
        let frames = 24000usize.div_ceil(32);
        assert_eq!(adx.len(), DATA + frames * FRAME_BYTES * 2 + FRAME_BYTES);
        assert_eq!(&adx[adx.len() - FRAME_BYTES..adx.len() - FRAME_BYTES + 2], &[0x80, 0x01]);
    }

    #[test]
    fn round_trip_quality() {
        for (rate, ch) in [(44100, 1), (48000, 2)] {
            let pcm = tone(rate, 1.0, ch);
            let adx = encode(&pcm, ch, rate, false);
            let back = decode(&adx).unwrap();
            assert_eq!(back.len(), pcm.len());
            let snr = snr_db(&pcm, &back);
            assert!(snr > 20.0, "snr {snr} at {rate}/{ch}");
        }
    }

    #[test]
    fn silence_and_loud_edges() {
        let mut pcm = vec![0i16; 1000];
        pcm.extend((0..1000).map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN }));
        let back = decode(&encode(&pcm, 1, 32000, false)).unwrap();
        assert!(back[..1000].iter().all(|&s| s.abs() < 4));
        assert_eq!(back.len(), pcm.len());
    }

    #[test]
    fn empty_input() {
        let adx = encode(&[], 1, 48000, true);
        assert_eq!(header(&adx), Some((1, 48000, 0, false)));
    }
}
