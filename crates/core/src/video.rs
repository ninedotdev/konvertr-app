//! Video converter: shells out to a native ffmpeg binary. Ported from
//! konvertr's ffmpeg.wasm implementation, but multithreaded-native, so we can
//! afford `-preset fast` x264 and libvpx-vp9 for webm.

use anyhow::{Context as _, Result, bail};
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoFormat {
    Mp4,
    WebM,
    Mov,
    Avi,
    Mkv,
    Flv,
    Wmv,
    Gif,
    Mp3,
    Wav,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 10] = [
        VideoFormat::Mp4,
        VideoFormat::WebM,
        VideoFormat::Mov,
        VideoFormat::Avi,
        VideoFormat::Mkv,
        VideoFormat::Flv,
        VideoFormat::Wmv,
        VideoFormat::Gif,
        VideoFormat::Mp3,
        VideoFormat::Wav,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4",
            VideoFormat::WebM => "WebM",
            VideoFormat::Mov => "MOV",
            VideoFormat::Avi => "AVI",
            VideoFormat::Mkv => "MKV",
            VideoFormat::Flv => "FLV",
            VideoFormat::Wmv => "WMV",
            VideoFormat::Gif => "GIF",
            VideoFormat::Mp3 => "MP3",
            VideoFormat::Wav => "WAV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "mp4",
            VideoFormat::WebM => "webm",
            VideoFormat::Mov => "mov",
            VideoFormat::Avi => "avi",
            VideoFormat::Mkv => "mkv",
            VideoFormat::Flv => "flv",
            VideoFormat::Wmv => "wmv",
            VideoFormat::Gif => "gif",
            VideoFormat::Mp3 => "mp3",
            VideoFormat::Wav => "wav",
        }
    }

    pub fn audio_only(self) -> bool {
        matches!(self, VideoFormat::Mp3 | VideoFormat::Wav)
    }

    /// Formats whose encoder takes a quality knob.
    pub fn supports_quality(self) -> bool {
        !matches!(self, VideoFormat::Gif | VideoFormat::Wav)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoQuality {
    High,
    Balanced,
    Small,
}

impl VideoQuality {
    pub const ALL: [VideoQuality; 3] = [
        VideoQuality::High,
        VideoQuality::Balanced,
        VideoQuality::Small,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VideoQuality::High => "high",
            VideoQuality::Balanced => "balanced",
            VideoQuality::Small => "smallest",
        }
    }

    fn crf(self) -> &'static str {
        match self {
            VideoQuality::High => "20",
            VideoQuality::Balanced => "23",
            VideoQuality::Small => "28",
        }
    }

    /// VP9's CRF scale runs colder than x264's — 31 is its "balanced" default.
    fn vp9_crf(self) -> &'static str {
        match self {
            VideoQuality::High => "28",
            VideoQuality::Balanced => "33",
            VideoQuality::Small => "38",
        }
    }

    fn audio_bitrate(self) -> &'static str {
        match self {
            VideoQuality::High => "192k",
            VideoQuality::Balanced => "128k",
            VideoQuality::Small => "96k",
        }
    }

    fn qscale(self) -> &'static str {
        match self {
            VideoQuality::High => "3",
            VideoQuality::Balanced => "5",
            VideoQuality::Small => "8",
        }
    }
}

/// Input extensions the video converter accepts.
pub const VIDEO_INPUT_EXTENSIONS: [&str; 15] = [
    "mp4", "webm", "mov", "avi", "mkv", "flv", "wmv", "gif", "m4v", "mpg", "mpeg", "3gp", "ogv",
    "ts", "mts",
];

pub fn is_supported_input(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_INPUT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Locate an ffmpeg binary: `KONVRT_FFMPEG` env override, next to the app
/// executable, the macOS bundle's Resources dir, the repo's `dist/bin/ffmpeg`
/// (so `cargo run` uses the bundled one), then each dir in PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KONVRT_FFMPEG") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [dir.join("ffmpeg"), dir.join("../Resources/ffmpeg")] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let dev = Path::new("dist/bin/ffmpeg");
    if dev.is_file() {
        return Some(dev.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("ffmpeg");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Conversion arguments (input through output); pure so it's unit-testable.
/// Global flags (`-y`, `-progress`, ...) are added by `convert_file`.
pub fn build_args(
    input: &Path,
    output: &Path,
    format: VideoFormat,
    quality: VideoQuality,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let input = input.to_string_lossy().into_owned();
    let output = output.to_string_lossy().into_owned();

    let x264 = |args: &mut Vec<String>| {
        args.extend([
            s("-c:v"),
            s("libx264"),
            s("-preset"),
            s("fast"),
            s("-crf"),
            s(quality.crf()),
            s("-pix_fmt"),
            s("yuv420p"),
        ]);
    };
    let aac = |args: &mut Vec<String>| {
        args.extend([s("-c:a"), s("aac"), s("-b:a"), s(quality.audio_bitrate())]);
    };

    let mut args = vec![s("-i"), input];
    match format {
        VideoFormat::Mp4 => {
            x264(&mut args);
            args.extend([s("-movflags"), s("+faststart")]);
            aac(&mut args);
        }
        VideoFormat::Mov | VideoFormat::Mkv | VideoFormat::Flv => {
            x264(&mut args);
            aac(&mut args);
        }
        VideoFormat::WebM => {
            args.extend([
                s("-c:v"),
                s("libvpx-vp9"),
                s("-crf"),
                s(quality.vp9_crf()),
                s("-b:v"),
                s("0"),
                s("-cpu-used"),
                s("4"),
                s("-row-mt"),
                s("1"),
                s("-c:a"),
                s("libopus"),
                s("-b:a"),
                s(quality.audio_bitrate()),
            ]);
        }
        VideoFormat::Avi => {
            args.extend([
                s("-c:v"),
                s("mpeg4"),
                s("-q:v"),
                s(quality.qscale()),
                s("-c:a"),
                s("libmp3lame"),
                s("-q:a"),
                s("4"),
            ]);
        }
        VideoFormat::Wmv => {
            args.extend([
                s("-c:v"),
                s("wmv2"),
                s("-q:v"),
                s(quality.qscale()),
                s("-c:a"),
                s("wmav2"),
                s("-b:a"),
                s(quality.audio_bitrate()),
            ]);
        }
        VideoFormat::Gif => {
            args.extend([
                s("-filter_complex"),
                s("fps=12,scale=480:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"),
                s("-loop"),
                s("0"),
            ]);
        }
        VideoFormat::Mp3 => {
            args.extend([s("-vn"), s("-c:a"), s("libmp3lame"), s("-q:a"), s("2")]);
        }
        VideoFormat::Wav => {
            args.extend([s("-vn"), s("-c:a"), s("pcm_s16le")]);
        }
    }
    args.push(output);
    args
}

/// Probe the input's duration by parsing `Duration: HH:MM:SS.cc` out of
/// `ffmpeg -i` stderr.
pub fn probe_duration_secs(ffmpeg: &Path, input: &Path) -> Option<f64> {
    let out = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-i")
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    parse_duration(&String::from_utf8_lossy(&out.stderr))
}

fn parse_duration(stderr: &str) -> Option<f64> {
    let ix = stderr.find("Duration: ")?;
    let rest = &stderr[ix + "Duration: ".len()..];
    let end = rest.find(',').unwrap_or(rest.len());
    parse_hms(rest[..end].trim())
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
pub struct VideoOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Sibling path for the converted file; never collides with the input or an
/// existing file (appends `-konverted`, then `-2`, `-3`, ...). Same semantics
/// as `crate::output_path` for images.
pub fn video_output_path(input: &Path, format: VideoFormat) -> PathBuf {
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
    format: VideoFormat,
    quality: VideoQuality,
    progress: &Arc<AtomicU8>,
) -> Result<VideoOutcome> {
    let in_size = std::fs::metadata(input)
        .with_context(|| format!("reading {}", input.display()))?
        .len();
    let out_path = video_output_path(input, format);
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
    Ok(VideoOutcome {
        out_path,
        in_size,
        out_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(format: VideoFormat, quality: VideoQuality) -> Vec<String> {
        build_args(
            Path::new("/in/a.mov"),
            Path::new("/in/a.out"),
            format,
            quality,
        )
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn build_args_mp4() {
        let a = args(VideoFormat::Mp4, VideoQuality::Balanced);
        assert_eq!(a[0], "-i");
        assert_eq!(a[1], "/in/a.mov");
        assert!(has_pair(&a, "-c:v", "libx264"));
        assert!(has_pair(&a, "-preset", "fast"));
        assert!(has_pair(&a, "-crf", "23"));
        assert!(has_pair(&a, "-movflags", "+faststart"));
        assert!(has_pair(&a, "-c:a", "aac"));
        assert!(has_pair(&a, "-b:a", "128k"));
        assert_eq!(a.last().unwrap(), "/in/a.out");
    }

    #[test]
    fn build_args_webm_vp9() {
        let a = args(VideoFormat::WebM, VideoQuality::High);
        assert!(has_pair(&a, "-c:v", "libvpx-vp9"));
        assert!(has_pair(&a, "-crf", "28"));
        assert!(has_pair(&a, "-b:v", "0"));
        assert!(has_pair(&a, "-row-mt", "1"));
        assert!(has_pair(&a, "-c:a", "libopus"));
    }

    #[test]
    fn build_args_audio_and_gif() {
        let mp3 = args(VideoFormat::Mp3, VideoQuality::Small);
        assert!(mp3.contains(&"-vn".to_string()));
        assert!(has_pair(&mp3, "-c:a", "libmp3lame"));
        assert!(has_pair(&mp3, "-q:a", "2"));

        let wav = args(VideoFormat::Wav, VideoQuality::High);
        assert!(has_pair(&wav, "-c:a", "pcm_s16le"));

        let gif = args(VideoFormat::Gif, VideoQuality::High);
        assert!(gif.iter().any(|a| a.contains("palettegen")));
        assert!(has_pair(&gif, "-loop", "0"));
        assert!(!gif.iter().any(|a| a == "-c:v"));
    }

    #[test]
    fn parses_duration_from_stderr() {
        let stderr = "Input #0, mov,mp4, from 'in.mp4':\n  Duration: 00:01:23.45, start: 0.000000, bitrate: 1000 kb/s\n";
        let d = parse_duration(stderr).unwrap();
        assert!((d - 83.45).abs() < 1e-9);
        assert_eq!(parse_duration("no duration here"), None);
        assert_eq!(parse_duration("  Duration: N/A, start:"), None);
    }

    #[test]
    fn parses_progress_lines() {
        // out_time_ms is microseconds
        assert_eq!(progress_percent("out_time_ms=50000000", 100.0), Some(50));
        assert_eq!(
            progress_percent("out_time=00:00:25.000000", 100.0),
            Some(25)
        );
        // clamped past the probed duration
        assert_eq!(progress_percent("out_time_ms=200000000", 100.0), Some(100));
        assert_eq!(progress_percent("frame=42", 100.0), None);
        assert_eq!(progress_percent("out_time_ms=1", 0.0), None);
    }

    #[test]
    fn detects_supported_inputs() {
        assert!(is_supported_input(Path::new("a/Clip.MP4")));
        assert!(is_supported_input(Path::new("a/clip.mts")));
        assert!(!is_supported_input(Path::new("a/photo.png")));
        assert!(!is_supported_input(Path::new("noext")));
    }

    #[test]
    fn output_path_avoids_input_collision() {
        let p = Path::new("/nope/clip.mp4");
        assert_eq!(
            video_output_path(p, VideoFormat::Mp4),
            Path::new("/nope/clip-konverted.mp4")
        );
        assert_eq!(
            video_output_path(p, VideoFormat::WebM),
            Path::new("/nope/clip.webm")
        );
    }
}
