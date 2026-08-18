//! Audio converter: shells out to the bundled ffmpeg, twin of `crate::video`.
//! Also accepts video inputs and extracts their audio track (`-vn` always).

use anyhow::{Context as _, Result, bail};
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::video::probe_duration_secs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioFormat {
    Mp3,
    Wav,
    Flac,
    Ogg,
    M4a,
    Opus,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 6] = [
        AudioFormat::Mp3,
        AudioFormat::Wav,
        AudioFormat::Flac,
        AudioFormat::Ogg,
        AudioFormat::M4a,
        AudioFormat::Opus,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "MP3",
            AudioFormat::Wav => "WAV",
            AudioFormat::Flac => "FLAC",
            AudioFormat::Ogg => "OGG",
            AudioFormat::M4a => "M4A",
            AudioFormat::Opus => "Opus",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "ogg",
            AudioFormat::M4a => "m4a",
            AudioFormat::Opus => "opus",
        }
    }

    /// Lossless formats (wav, flac) take no quality knob.
    pub fn supports_quality(self) -> bool {
        !matches!(self, AudioFormat::Wav | AudioFormat::Flac)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioQuality {
    High,
    Balanced,
    Small,
}

impl AudioQuality {
    pub const ALL: [AudioQuality; 3] = [
        AudioQuality::High,
        AudioQuality::Balanced,
        AudioQuality::Small,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AudioQuality::High => "high",
            AudioQuality::Balanced => "balanced",
            AudioQuality::Small => "smallest",
        }
    }

    fn mp3_q(self) -> &'static str {
        match self {
            AudioQuality::High => "0",
            AudioQuality::Balanced => "2",
            AudioQuality::Small => "5",
        }
    }

    fn vorbis_q(self) -> &'static str {
        match self {
            AudioQuality::High => "7",
            AudioQuality::Balanced => "5",
            AudioQuality::Small => "3",
        }
    }

    fn opus_bitrate(self) -> &'static str {
        match self {
            AudioQuality::High => "192k",
            AudioQuality::Balanced => "128k",
            AudioQuality::Small => "96k",
        }
    }

    fn aac_bitrate(self) -> &'static str {
        match self {
            AudioQuality::High => "256k",
            AudioQuality::Balanced => "160k",
            AudioQuality::Small => "96k",
        }
    }
}

/// Audio inputs, plus common video containers we can extract audio from.
pub const AUDIO_INPUT_EXTENSIONS: [&str; 15] = [
    "mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma", "aiff", "aif", "mp4", "mov", "mkv",
    "webm", "avi",
];

pub fn is_supported_input(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_INPUT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Conversion arguments (input through output); pure so it's unit-testable.
/// Global flags (`-y`, `-progress`, ...) are added by `convert_file`.
pub fn build_args(
    input: &Path,
    output: &Path,
    format: AudioFormat,
    quality: AudioQuality,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut args = vec![s("-i"), input.to_string_lossy().into_owned(), s("-vn")];
    match format {
        AudioFormat::Mp3 => {
            args.extend([s("-c:a"), s("libmp3lame"), s("-q:a"), s(quality.mp3_q())]);
        }
        AudioFormat::Wav => {
            args.extend([s("-c:a"), s("pcm_s16le")]);
        }
        AudioFormat::Flac => {
            args.extend([s("-c:a"), s("flac")]);
        }
        AudioFormat::Ogg => {
            args.extend([s("-c:a"), s("libvorbis"), s("-q:a"), s(quality.vorbis_q())]);
        }
        AudioFormat::M4a => {
            args.extend([s("-c:a"), s("aac"), s("-b:a"), s(quality.aac_bitrate())]);
        }
        AudioFormat::Opus => {
            args.extend([
                s("-c:a"),
                s("libopus"),
                s("-b:a"),
                s(quality.opus_bitrate()),
            ]);
        }
    }
    args.push(output.to_string_lossy().into_owned());
    args
}

/// Parse `HH:MM:SS.cc` into seconds.
fn parse_hms(s: &str) -> Option<f64> {
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.trim().parse().ok()?;
    let m: f64 = parts.next()?.trim().parse().ok()?;
    let sec: f64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Percent complete from one `-progress pipe:1` line, given the total
/// duration. ffmpeg's `out_time_ms` is (despite the name) in microseconds.
fn progress_percent(line: &str, duration_secs: f64) -> Option<u8> {
    let secs = if let Some(v) = line.strip_prefix("out_time_ms=") {
        v.trim().parse::<f64>().ok()? / 1_000_000.0
    } else {
        let v = line.strip_prefix("out_time=")?;
        parse_hms(v.trim())?
    };
    if duration_secs <= 0.0 {
        return None;
    }
    Some((secs / duration_secs * 100.0).clamp(0.0, 100.0) as u8)
}

#[derive(Clone, Debug)]
pub struct AudioOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Sibling path for the converted file; never collides with the input or an
/// existing file (appends `-konverted`, then `-2`, `-3`, ...). Same semantics
/// as `crate::output_path` for images.
pub fn audio_output_path(input: &Path, format: AudioFormat) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let ext = format.extension();
    let sibling = |name: String| input.with_file_name(name);

    let mut candidate = sibling(format!("{stem}.{ext}"));
    if candidate == input {
        candidate = sibling(format!("{stem}-konverted.{ext}"));
    }
    let base = candidate
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(stem)
        .to_string();
    let mut n = 2;
    while candidate.exists() {
        candidate = sibling(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    candidate
}

/// Convert `input` next to itself, blocking until ffmpeg exits. Meant for a
/// background thread; live percent (0..=100) is stored into `progress`.
pub fn convert_file(
    ffmpeg: &Path,
    input: &Path,
    format: AudioFormat,
    quality: AudioQuality,
    progress: &Arc<AtomicU8>,
) -> Result<AudioOutcome> {
    let in_size = std::fs::metadata(input)
        .with_context(|| format!("reading {}", input.display()))?
        .len();
    let out_path = audio_output_path(input, format);
    let duration = probe_duration_secs(ffmpeg, input);

    progress.store(0, Ordering::Relaxed);
    let mut child = Command::new(ffmpeg)
        .args(["-y", "-nostats", "-progress", "pipe:1"])
        .args(build_args(input, &out_path, format, quality))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("launching {}", ffmpeg.display()))?;

    // Drain stderr on its own thread so a chatty ffmpeg can't fill the pipe
    // and deadlock against our stdout reads.
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        stderr.read_to_string(&mut buf).ok();
        buf
    });

    let stdout = child.stdout.take().expect("stdout piped");
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.trim() == "progress=end" {
            progress.store(100, Ordering::Relaxed);
        } else if let Some(pct) = duration.and_then(|d| progress_percent(&line, d)) {
            progress.store(pct, Ordering::Relaxed);
        }
    }

    let status = child.wait().context("waiting for ffmpeg")?;
    let stderr_text = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        std::fs::remove_file(&out_path).ok();
        let detail = stderr_text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error")
            .trim()
            .to_string();
        bail!("ffmpeg failed: {detail}");
    }

    let out_size = std::fs::metadata(&out_path)
        .with_context(|| format!("reading {}", out_path.display()))?
        .len();
    if out_size == 0 {
        std::fs::remove_file(&out_path).ok();
        bail!("conversion produced no output");
    }
    progress.store(100, Ordering::Relaxed);
    Ok(AudioOutcome {
        out_path,
        in_size,
        out_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(format: AudioFormat, quality: AudioQuality) -> Vec<String> {
        build_args(
            Path::new("/in/a.mp4"),
            Path::new("/in/a.out"),
            format,
            quality,
        )
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn build_args_always_drops_video() {
        for format in AudioFormat::ALL {
            let a = args(format, AudioQuality::Balanced);
            assert_eq!(a[0], "-i");
            assert_eq!(a[1], "/in/a.mp4");
            assert!(a.contains(&"-vn".to_string()), "{}", format.label());
            assert_eq!(a.last().unwrap(), "/in/a.out");
        }
    }

    #[test]
    fn build_args_mp3_qualities() {
        assert!(has_pair(
            &args(AudioFormat::Mp3, AudioQuality::High),
            "-q:a",
            "0"
        ));
        assert!(has_pair(
            &args(AudioFormat::Mp3, AudioQuality::Balanced),
            "-q:a",
            "2"
        ));
        assert!(has_pair(
            &args(AudioFormat::Mp3, AudioQuality::Small),
            "-q:a",
            "5"
        ));
        assert!(has_pair(
            &args(AudioFormat::Mp3, AudioQuality::High),
            "-c:a",
            "libmp3lame"
        ));
    }

    #[test]
    fn build_args_ogg_and_opus() {
        let ogg = args(AudioFormat::Ogg, AudioQuality::High);
        assert!(has_pair(&ogg, "-c:a", "libvorbis"));
        assert!(has_pair(&ogg, "-q:a", "7"));
        assert!(has_pair(
            &args(AudioFormat::Ogg, AudioQuality::Small),
            "-q:a",
            "3"
        ));

        let opus = args(AudioFormat::Opus, AudioQuality::Balanced);
        assert!(has_pair(&opus, "-c:a", "libopus"));
        assert!(has_pair(&opus, "-b:a", "128k"));
    }

    #[test]
    fn build_args_m4a_and_lossless() {
        let m4a = args(AudioFormat::M4a, AudioQuality::High);
        assert!(has_pair(&m4a, "-c:a", "aac"));
        assert!(has_pair(&m4a, "-b:a", "256k"));

        let wav = args(AudioFormat::Wav, AudioQuality::Small);
        assert!(has_pair(&wav, "-c:a", "pcm_s16le"));
        assert!(!wav.contains(&"-b:a".to_string()));

        let flac = args(AudioFormat::Flac, AudioQuality::Small);
        assert!(has_pair(&flac, "-c:a", "flac"));
        assert!(!flac.contains(&"-q:a".to_string()));
    }

    #[test]
    fn quality_support() {
        assert!(!AudioFormat::Wav.supports_quality());
        assert!(!AudioFormat::Flac.supports_quality());
        assert!(AudioFormat::Mp3.supports_quality());
        assert!(AudioFormat::Opus.supports_quality());
    }

    #[test]
    fn detects_supported_inputs() {
        assert!(is_supported_input(Path::new("a/Track.MP3")));
        assert!(is_supported_input(Path::new("a/take.aiff")));
        assert!(is_supported_input(Path::new("a/movie.mp4"))); // extract audio
        assert!(!is_supported_input(Path::new("a/photo.png")));
        assert!(!is_supported_input(Path::new("noext")));
    }

    #[test]
    fn parses_progress_lines() {
        assert_eq!(progress_percent("out_time_ms=50000000", 100.0), Some(50));
        assert_eq!(
            progress_percent("out_time=00:00:25.000000", 100.0),
            Some(25)
        );
        assert_eq!(progress_percent("frame=42", 100.0), None);
    }

    #[test]
    fn output_path_avoids_input_collision() {
        let p = Path::new("/nope/track.mp3");
        assert_eq!(
            audio_output_path(p, AudioFormat::Mp3),
            Path::new("/nope/track-konverted.mp3")
        );
        assert_eq!(
            audio_output_path(p, AudioFormat::Flac),
            Path::new("/nope/track.flac")
        );
    }
}
