//! Video studio: ffmpeg utilities beyond plain format conversion — compress
//! to a target size (two-pass), lossless trim, GIF studio, frame extraction.
//! Same shell-out machinery as `crate::video`.

use anyhow::{Context as _, Result, bail};
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::video::probe_duration_secs;

#[derive(Clone, Debug)]
pub struct StudioOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Share-target presets for the compress UI: (label, megabytes).
pub const COMPRESS_PRESETS: [(&str, f64); 3] = [
    ("Discord 10 MB", 10.0),
    ("WhatsApp 16 MB", 16.0),
    ("Email 25 MB", 25.0),
];

// ---------------------------------------------------------------- output path

/// Sibling path `{stem}-{suffix}.{ext}` (or `{stem}.{ext}` for an empty
/// suffix); never collides with the input or an existing file — same
/// semantics as the image/video output paths.
pub fn studio_output_path(input: &Path, suffix: &str, ext: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let sibling = |name: String| input.with_file_name(name);

    let named = if suffix.is_empty() {
        format!("{stem}.{ext}")
    } else {
        format!("{stem}-{suffix}.{ext}")
    };
    let mut candidate = sibling(named);
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

// ------------------------------------------------------------------ runner

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

/// Fraction complete (0..=1) from one `-progress pipe:1` line. ffmpeg's
/// `out_time_ms` is (despite the name) in microseconds.
fn progress_fraction(line: &str, duration_secs: f64) -> Option<f64> {
    let secs = if let Some(v) = line.strip_prefix("out_time_ms=") {
        v.trim().parse::<f64>().ok()? / 1_000_000.0
    } else {
        let v = line.strip_prefix("out_time=")?;
        parse_hms(v.trim())?
    };
    if duration_secs <= 0.0 {
        return None;
    }
    Some((secs / duration_secs).clamp(0.0, 1.0))
}

/// Run ffmpeg with `args` (input through output), blocking. Live progress is
/// mapped into `pct_range` (e.g. (0, 50) for pass 1 of 2) against
/// `duration_secs` and stored into `progress`.
pub fn run(
    ffmpeg: &Path,
    args: &[String],
    progress: &Arc<AtomicU8>,
    duration_secs: Option<f64>,
    pct_range: (u8, u8),
) -> Result<()> {
    let (lo, hi) = pct_range;
    progress.store(lo, Ordering::Relaxed);
    let mut child = Command::new(ffmpeg)
        .args(["-y", "-nostats", "-progress", "pipe:1"])
        .args(args)
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
            progress.store(hi, Ordering::Relaxed);
        } else if let Some(frac) = duration_secs.and_then(|d| progress_fraction(&line, d)) {
            let pct = lo as f64 + frac * (hi - lo) as f64;
            progress.store(pct as u8, Ordering::Relaxed);
        }
    }

    let status = child.wait().context("waiting for ffmpeg")?;
    let stderr_text = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        let detail = stderr_text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error")
            .trim()
            .to_string();
        bail!("ffmpeg failed: {detail}");
    }
    progress.store(hi, Ordering::Relaxed);
    Ok(())
}

fn finish(input: &Path, out_path: PathBuf) -> Result<StudioOutcome> {
    let in_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let out_size = std::fs::metadata(&out_path)
        .with_context(|| format!("reading {}", out_path.display()))?
        .len();
    if out_size == 0 {
        std::fs::remove_file(&out_path).ok();
        bail!("produced no output");
    }
    Ok(StudioOutcome {
        out_path,
        in_size,
        out_size,
    })
}

// ------------------------------------------------------------ 1. compress

/// Video bitrate (kbps) that lands a `duration_secs` clip at `target_mb`,
/// after `audio_kbps` audio and a ~4% container-overhead margin.
pub fn video_bitrate_kbps(target_mb: f64, duration_secs: f64, audio_kbps: u32) -> Result<u32> {
    if duration_secs <= 0.0 {
        bail!("unknown duration");
    }
    let total_kbps = target_mb * 1024.0 * 1024.0 * 8.0 / 1000.0 / duration_secs * 0.96;
    let video = total_kbps - audio_kbps as f64;
    if video < 100.0 {
        bail!("target too small for this duration");
    }
    Ok(video as u32)
}

/// Args for one pass of the two-pass target-size encode (`pass` is 1 or 2).
/// Pass 1 analyzes to the passlog and discards output; pass 2 writes `output`.
pub fn compress_args(
    input: &Path,
    output: &Path,
    target_mb: f64,
    duration_secs: f64,
    audio_kbps: u32,
    pass: u8,
) -> Result<Vec<String>> {
    let v = video_bitrate_kbps(target_mb, duration_secs, audio_kbps)?;
    let s = |v: &str| v.to_string();
    let passlog = output.with_extension("passlog");
    let mut args = vec![
        s("-i"),
        input.to_string_lossy().into_owned(),
        s("-c:v"),
        s("libx264"),
        s("-preset"),
        s("fast"),
        s("-b:v"),
        format!("{v}k"),
        s("-pix_fmt"),
        s("yuv420p"),
        s("-pass"),
        pass.to_string(),
        s("-passlogfile"),
        passlog.to_string_lossy().into_owned(),
    ];
    if pass == 1 {
        args.extend([s("-an"), s("-f"), s("null"), s("/dev/null")]);
    } else {
        args.extend([
            s("-c:a"),
            s("aac"),
            s("-b:a"),
            format!("{audio_kbps}k"),
            s("-movflags"),
            s("+faststart"),
            output.to_string_lossy().into_owned(),
        ]);
    }
    Ok(args)
}

/// Two-pass compress to `target_mb`; pass 1 maps to 0..50%, pass 2 to 50..100%.
pub fn compress_file(
    ffmpeg: &Path,
    input: &Path,
    target_mb: f64,
    audio_kbps: u32,
    progress: &Arc<AtomicU8>,
) -> Result<StudioOutcome> {
    let duration = probe_duration_secs(ffmpeg, input)
        .ok_or_else(|| anyhow::anyhow!("could not read the video's duration"))?;
    let out_path = studio_output_path(input, "compressed", "mp4");
    let pass1 = compress_args(input, &out_path, target_mb, duration, audio_kbps, 1)?;
    let pass2 = compress_args(input, &out_path, target_mb, duration, audio_kbps, 2)?;

    let result = run(ffmpeg, &pass1, progress, Some(duration), (0, 50))
        .and_then(|_| run(ffmpeg, &pass2, progress, Some(duration), (50, 100)));

    let passlog = out_path.with_extension("passlog");
    std::fs::remove_file(format!("{}-0.log", passlog.display())).ok();
    std::fs::remove_file(format!("{}-0.log.mbtree", passlog.display())).ok();

    if let Err(e) = result {
        std::fs::remove_file(&out_path).ok();
        return Err(e);
    }
    finish(input, out_path)
}

// ---------------------------------------------------------------- 2. trim

/// Lossless trim: `-ss` before `-i` (fast keyframe seek), stream copy.
/// With input seeking, output timestamps restart at zero, so `-to` carries
/// the clip length (`end - start`).
pub fn trim_args(
    input: &Path,
    output: &Path,
    start_secs: f64,
    end_secs: Option<f64>,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut args = vec![
        s("-ss"),
        start_secs.to_string(),
        s("-i"),
        input.to_string_lossy().into_owned(),
    ];
    if let Some(end) = end_secs {
        args.extend([s("-to"), (end - start_secs).max(0.0).to_string()]);
    }
    args.extend([
        s("-c"),
        s("copy"),
        s("-avoid_negative_ts"),
        s("make_zero"),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

pub fn trim_file(
    ffmpeg: &Path,
    input: &Path,
    start_secs: f64,
    end_secs: Option<f64>,
    progress: &Arc<AtomicU8>,
) -> Result<StudioOutcome> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    let out_path = studio_output_path(input, "trim", &ext);
    let total = probe_duration_secs(ffmpeg, input);
    let clip = end_secs.or(total).map(|end| (end - start_secs).max(0.0));
    let args = trim_args(input, &out_path, start_secs, end_secs);
    if let Err(e) = run(ffmpeg, &args, progress, clip, (0, 100)) {
        std::fs::remove_file(&out_path).ok();
        return Err(e);
    }
    finish(input, out_path)
}

/// Parse "90", "1:30", or "01:02:03.5" into seconds.
pub fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for part in parts {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

/// Inverse of `parse_timestamp`: "1:30", "1:02:03.5" (tenths only when
/// fractional).
pub fn format_timestamp(secs: f64) -> String {
    let clamped = secs.max(0.0);
    let mut whole = clamped.floor() as u64;
    let mut tenths = ((clamped - whole as f64) * 10.0).round() as u64;
    if tenths >= 10 {
        whole += 1;
        tenths = 0;
    }
    let (h, m, s) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    let mut out = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if tenths > 0 {
        out.push_str(&format!(".{tenths}"));
    }
    out
}

// ----------------------------------------------------------------- 3. gif

#[derive(Clone, Copy, Debug)]
pub struct GifOptions {
    pub start: f64,
    pub end: Option<f64>,
    pub fps: u32,
    pub width: u32,
    pub loop_forever: bool,
    pub dither: bool,
}

/// Single-pass palettegen/paletteuse GIF.
pub fn gif_args(input: &Path, output: &Path, opts: &GifOptions) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut args = vec![
        s("-ss"),
        opts.start.to_string(),
        s("-i"),
        input.to_string_lossy().into_owned(),
    ];
    if let Some(end) = opts.end {
        args.extend([s("-to"), (end - opts.start).max(0.0).to_string()]);
    }
    let dither = if opts.dither {
        "dither=bayer:bayer_scale=5"
    } else {
        "dither=none"
    };
    args.extend([
        s("-filter_complex"),
        format!(
            "fps={},scale={}:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse={dither}",
            opts.fps, opts.width
        ),
        s("-loop"),
        if opts.loop_forever { s("0") } else { s("-1") },
        output.to_string_lossy().into_owned(),
    ]);
    args
}

pub fn gif_file(
    ffmpeg: &Path,
    input: &Path,
    opts: &GifOptions,
    progress: &Arc<AtomicU8>,
) -> Result<StudioOutcome> {
    let out_path = studio_output_path(input, "", "gif");
    let total = probe_duration_secs(ffmpeg, input);
    let clip = opts.end.or(total).map(|end| (end - opts.start).max(0.0));
    let args = gif_args(input, &out_path, opts);
    if let Err(e) = run(ffmpeg, &args, progress, clip, (0, 100)) {
        std::fs::remove_file(&out_path).ok();
        return Err(e);
    }
    finish(input, out_path)
}

// --------------------------------------------------------------- 4. frames

/// One PNG frame at `at_secs` (fast keyframe seek).
pub fn frame_args(input: &Path, output_png: &Path, at_secs: f64) -> Vec<String> {
    let s = |v: &str| v.to_string();
    vec![
        s("-ss"),
        at_secs.to_string(),
        s("-i"),
        input.to_string_lossy().into_owned(),
        s("-frames:v"),
        s("1"),
        output_png.to_string_lossy().into_owned(),
    ]
}

pub fn frame_file(
    ffmpeg: &Path,
    input: &Path,
    at_secs: f64,
    progress: &Arc<AtomicU8>,
) -> Result<StudioOutcome> {
    let out_path = studio_output_path(input, "frame", "png");
    let args = frame_args(input, &out_path, at_secs);
    if let Err(e) = run(ffmpeg, &args, progress, None, (0, 100)) {
        std::fs::remove_file(&out_path).ok();
        return Err(e);
    }
    finish(input, out_path)
}

/// Thumbnail grid of the whole video: cols×rows frames sampled evenly across
/// `duration_secs`, each `width` px wide, tiled into one PNG.
pub fn contact_sheet_args(
    input: &Path,
    output_png: &Path,
    cols: u32,
    rows: u32,
    width: u32,
    duration_secs: f64,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let interval = duration_secs / (cols * rows) as f64;
    vec![
        s("-i"),
        input.to_string_lossy().into_owned(),
        s("-vf"),
        format!(
            "select='isnan(prev_selected_t)+gte(t-prev_selected_t\\,{interval:.3})',scale={width}:-1,tile={cols}x{rows}"
        ),
        s("-frames:v"),
        s("1"),
        s("-fps_mode"),
        s("vfr"),
        output_png.to_string_lossy().into_owned(),
    ]
}

pub fn contact_sheet_file(
    ffmpeg: &Path,
    input: &Path,
    cols: u32,
    rows: u32,
    width: u32,
    progress: &Arc<AtomicU8>,
) -> Result<StudioOutcome> {
    let duration = probe_duration_secs(ffmpeg, input)
        .ok_or_else(|| anyhow::anyhow!("could not read the video's duration"))?;
    let out_path = studio_output_path(input, "sheet", "png");
    let args = contact_sheet_args(input, &out_path, cols, rows, width, duration);
    if let Err(e) = run(ffmpeg, &args, progress, Some(duration), (0, 100)) {
        std::fs::remove_file(&out_path).ok();
        return Err(e);
    }
    finish(input, out_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn bitrate_math() {
        // 10 MB over 60s: 83886.08/60 kbps total, ×0.96, −128k audio ≈ 1214
        let v = video_bitrate_kbps(10.0, 60.0, 128).unwrap();
        assert!((1210..=1218).contains(&v), "{v}");
        // longer video, same target → lower bitrate
        assert!(video_bitrate_kbps(10.0, 120.0, 128).unwrap() < v);
    }

    #[test]
    fn bitrate_refuses_tiny_targets() {
        let err = video_bitrate_kbps(1.0, 3600.0, 128).unwrap_err();
        assert!(err.to_string().contains("target too small"), "{err}");
        assert!(video_bitrate_kbps(10.0, 0.0, 128).is_err());
    }

    #[test]
    fn timestamp_parsing() {
        assert_eq!(parse_timestamp("90"), Some(90.0));
        assert_eq!(parse_timestamp("1:30"), Some(90.0));
        assert_eq!(parse_timestamp("01:02:03.5"), Some(3723.5));
        assert_eq!(parse_timestamp(" 0:05 "), Some(5.0));
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("a:b"), None);
        assert_eq!(parse_timestamp("1:2:3:4"), None);
    }

    #[test]
    fn timestamp_format_round_trips() {
        assert_eq!(format_timestamp(90.0), "1:30");
        assert_eq!(format_timestamp(3723.5), "1:02:03.5");
        assert_eq!(format_timestamp(0.0), "0:00");
        for secs in [0.0, 5.0, 90.0, 3599.9, 3723.5, 7325.0] {
            let round = parse_timestamp(&format_timestamp(secs)).unwrap();
            assert!((round - secs).abs() < 0.06, "{secs} → {round}");
        }
    }

    #[test]
    fn compress_args_two_pass() {
        let input = Path::new("/in/a.mov");
        let output = Path::new("/in/a-compressed.mp4");
        let p1 = compress_args(input, output, 10.0, 60.0, 128, 1).unwrap();
        let p2 = compress_args(input, output, 10.0, 60.0, 128, 2).unwrap();
        assert!(has_pair(&p1, "-pass", "1"));
        assert!(has_pair(&p1, "-f", "null"));
        assert!(p1.contains(&"-an".to_string()));
        assert!(!p1.contains(&"/in/a-compressed.mp4".to_string()));
        assert!(has_pair(&p2, "-pass", "2"));
        assert!(has_pair(&p2, "-c:a", "aac"));
        assert!(has_pair(&p2, "-b:a", "128k"));
        assert_eq!(p2.last().unwrap(), "/in/a-compressed.mp4");
        // both passes agree on the exact video bitrate
        let v = format!("{}k", video_bitrate_kbps(10.0, 60.0, 128).unwrap());
        assert!(has_pair(&p1, "-b:v", &v));
        assert!(has_pair(&p2, "-b:v", &v));
    }

    #[test]
    fn trim_args_stream_copy() {
        let a = trim_args(
            Path::new("/in/a.mp4"),
            Path::new("/in/a-trim.mp4"),
            90.0,
            Some(120.0),
        );
        assert_eq!(&a[0..2], &["-ss".to_string(), "90".to_string()]);
        assert!(has_pair(&a, "-to", "30")); // relative to the seeked start
        assert!(has_pair(&a, "-c", "copy"));
        assert!(has_pair(&a, "-avoid_negative_ts", "make_zero"));

        let open_ended = trim_args(Path::new("/in/a.mp4"), Path::new("/in/o.mp4"), 5.0, None);
        assert!(!open_ended.contains(&"-to".to_string()));
    }

    #[test]
    fn gif_filter_string() {
        let opts = GifOptions {
            start: 1.0,
            end: Some(4.0),
            fps: 12,
            width: 480,
            loop_forever: true,
            dither: true,
        };
        let a = gif_args(Path::new("/in/a.mp4"), Path::new("/in/a.gif"), &opts);
        let filter = &a[a.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(filter.starts_with("fps=12,scale=480:-1:flags=lanczos"));
        assert!(filter.contains("split[a][b];[a]palettegen[p];[b][p]paletteuse"));
        assert!(filter.contains("dither=bayer"));
        assert!(has_pair(&a, "-loop", "0"));
        assert!(has_pair(&a, "-to", "3"));

        let once = GifOptions {
            loop_forever: false,
            dither: false,
            ..opts
        };
        let a = gif_args(Path::new("/in/a.mp4"), Path::new("/in/a.gif"), &once);
        assert!(has_pair(&a, "-loop", "-1"));
        assert!(a.iter().any(|x| x.contains("dither=none")));
    }

    #[test]
    fn frame_and_sheet_args() {
        let f = frame_args(Path::new("/in/a.mp4"), Path::new("/in/a-frame.png"), 12.5);
        assert_eq!(&f[0..2], &["-ss".to_string(), "12.5".to_string()]);
        assert!(has_pair(&f, "-frames:v", "1"));
        assert_eq!(f.last().unwrap(), "/in/a-frame.png");

        let s = contact_sheet_args(
            Path::new("/in/a.mp4"),
            Path::new("/in/a-sheet.png"),
            4,
            4,
            320,
            160.0,
        );
        let vf = &s[s.iter().position(|x| x == "-vf").unwrap() + 1];
        assert!(vf.contains("tile=4x4"));
        assert!(vf.contains("scale=320:-1"));
        assert!(vf.contains("10.000")); // 160s / 16 frames
        assert!(has_pair(&s, "-frames:v", "1"));
    }

    #[test]
    fn output_path_naming() {
        let p = Path::new("/nope/clip.mp4");
        assert_eq!(
            studio_output_path(p, "compressed", "mp4"),
            Path::new("/nope/clip-compressed.mp4")
        );
        assert_eq!(
            studio_output_path(p, "", "gif"),
            Path::new("/nope/clip.gif")
        );
        // empty suffix + same extension must not collide with the input
        assert_eq!(
            studio_output_path(p, "", "mp4"),
            Path::new("/nope/clip-konverted.mp4")
        );
    }
}
