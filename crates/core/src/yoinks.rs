//! loader: video downloader shelling out to a native yt-dlp binary. Ported
//! from pablostanley/yoinks' ytdlp.ts: probe with `-J`, build quality choices
//! from the format list, download with a structured progress template.

use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Locate a yt-dlp binary: `KONVRT_YTDLP` env override, next to the app
/// executable, the macOS bundle's Resources dir, `dist/bin/yt-dlp` relative to
/// the cwd (dev builds), then each dir in PATH. Mirrors `find_ffmpeg`.
pub fn find_ytdlp() -> Option<PathBuf> {
    const YTDLP_EXE: &str = if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    if let Some(p) = std::env::var_os("KONVRT_YTDLP") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for candidate in [
            dir.join(YTDLP_EXE),
            dir.join("../Resources").join(YTDLP_EXE),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let dev = Path::new("dist/bin").join(YTDLP_EXE);
    if dev.is_file() {
        return Some(dev);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(YTDLP_EXE);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Where downloads land: `$HOME/Downloads` (cwd as a last resort).
pub fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Quick sanity check before probing: http(s) scheme + a dotted host.
pub fn is_probable_url(s: &str) -> bool {
    let s = s.trim();
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !s.contains(char::is_whitespace)
}

/// Self-update a standalone yt-dlp binary (`yt-dlp -U`); returns the last
/// stdout line ("yt-dlp is up to date" / "Updated yt-dlp to ...").
pub fn update_ytdlp(ytdlp: &Path) -> Result<String> {
    let out = Command::new(ytdlp)
        .arg("-U")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("launching {}", ytdlp.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp -U failed: {}", clean_error(&stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("updated")
        .trim()
        .to_string())
}

// --- Probe ---

#[derive(Clone, Debug)]
pub struct DownloadChoice {
    /// e.g. "1080p · mp4 · ~120 MB", "audio only · mp3 · ~4.2 MB".
    pub label: String,
    pub audio: bool,
    /// The `-f ...` (+ merge/extract) args for this choice.
    pub format_args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ProbeResult {
    pub title: String,
    pub uploader: Option<String>,
    pub duration_secs: Option<f64>,
    pub choices: Vec<DownloadChoice>,
    /// Raw -J output saved to disk so downloads can skip re-extraction.
    pub info_json_path: PathBuf,
}

static INFO_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `yt-dlp -J` the url and build the quality choices. Blocking; run on a
/// background executor.
pub fn probe(ytdlp: &Path, url: &str) -> Result<ProbeResult> {
    let out = Command::new(ytdlp)
        .args(["-J", "--no-playlist", "--no-warnings"])
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("launching {}", ytdlp.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = clean_error(&stderr);
        if detail.is_empty() {
            bail!("yt-dlp exited with {}", out.status);
        }
        bail!("{detail}");
    }

    let info: Value =
        serde_json::from_slice(&out.stdout).context("could not parse video info from yt-dlp")?;

    let info_json_path = std::env::temp_dir().join(format!(
        "konvrt-info-{}-{}.json",
        std::process::id(),
        INFO_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&info_json_path, &out.stdout)
        .with_context(|| format!("writing {}", info_json_path.display()))?;

    Ok(ProbeResult {
        title: info
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("untitled")
            .to_string(),
        uploader: info
            .get("uploader")
            .and_then(Value::as_str)
            .map(str::to_string),
        duration_secs: info.get("duration").and_then(Value::as_f64),
        choices: build_choices(&info),
        info_json_path,
    })
}

const MAX_VIDEO_CHOICES: usize = 8;

fn fmt_f64(v: Option<f64>) -> f64 {
    v.unwrap_or(0.0)
}

/// Port of ytdlp.ts buildChoices: one chip per available height (best
/// candidate per height scored by tbr + mp4/avc bonuses), a "best available"
/// fallback when the format list carries no heights, and an mp3 choice last.
pub fn build_choices(info: &Value) -> Vec<DownloadChoice> {
    let empty = Vec::new();
    let formats = info
        .get("formats")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut choices = Vec::new();

    let has = |f: &Value, key: &str| {
        f.get(key)
            .and_then(Value::as_str)
            .is_some_and(|v| v != "none")
    };
    let size_of = |f: &Value| {
        f.get("filesize")
            .and_then(Value::as_f64)
            .or_else(|| f.get("filesize_approx").and_then(Value::as_f64))
    };

    let audio_only: Vec<&Value> = formats
        .iter()
        .filter(|f| has(f, "acodec") && !has(f, "vcodec"))
        .collect();
    let best_audio = audio_only.iter().max_by(|a, b| {
        let rate = |f: &&&Value| {
            fmt_f64(
                f.get("abr")
                    .and_then(Value::as_f64)
                    .or_else(|| f.get("tbr").and_then(Value::as_f64)),
            )
        };
        rate(a).total_cmp(&rate(b))
    });
    let audio_size = best_audio.and_then(|f| size_of(f));

    let videos: Vec<&Value> = formats
        .iter()
        .filter(|f| has(f, "vcodec") && f.get("height").and_then(Value::as_u64).is_some())
        .collect();
    let mut heights: Vec<u64> = videos
        .iter()
        .filter_map(|f| f.get("height").and_then(Value::as_u64))
        .collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();

    for &height in heights.iter().take(MAX_VIDEO_CHOICES) {
        let best = videos
            .iter()
            .filter(|f| f.get("height").and_then(Value::as_u64) == Some(height))
            .max_by(|a, b| score_video(a).total_cmp(&score_video(b)));
        let Some(best) = best else { continue };
        let muxed = has(best, "acodec");
        let size = fmt_f64(size_of(best)) + if muxed { 0.0 } else { fmt_f64(audio_size) };
        let size_label = if size > 0.0 {
            format!(" · ~{}", format_bytes(size))
        } else {
            String::new()
        };
        choices.push(DownloadChoice {
            label: format!("{height}p · mp4{size_label}"),
            audio: false,
            format_args: vec![
                "-f".into(),
                format!("bv*[height={height}]+ba/b[height={height}]/bv*[height<={height}]+ba/b"),
                "--merge-output-format".into(),
                "mp4".into(),
            ],
        });
    }

    if choices.is_empty() {
        choices.push(DownloadChoice {
            label: "best available · mp4".into(),
            audio: false,
            format_args: vec![
                "-f".into(),
                "bv*+ba/b".into(),
                "--merge-output-format".into(),
                "mp4".into(),
            ],
        });
    }

    let audio_size_label = match audio_size {
        Some(s) if s > 0.0 => format!(" · ~{}", format_bytes(s)),
        _ => String::new(),
    };
    choices.push(DownloadChoice {
        label: format!("audio only · mp3{audio_size_label}"),
        audio: true,
        format_args: vec![
            "-f".into(),
            "ba/b".into(),
            "-x".into(),
            "--audio-format".into(),
            "mp3".into(),
            "--audio-quality".into(),
            "0".into(),
        ],
    });

    choices
}

fn score_video(f: &Value) -> f64 {
    let mut score = fmt_f64(f.get("tbr").and_then(Value::as_f64));
    if f.get("ext").and_then(Value::as_str) == Some("mp4") {
        score += 10_000.0;
    }
    if f.get("vcodec")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with("avc"))
    {
        score += 5_000.0;
    }
    score
}

/// "120 MB"-style label (port of yoinks' formatBytes).
pub fn format_bytes(bytes: f64) -> String {
    if !bytes.is_finite() || bytes <= 0.0 {
        return String::new();
    }
    let units = ["B", "KB", "MB", "GB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 10.0 || unit == 0 {
        format!("{} {}", value.round() as u64, units[unit])
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

// --- Download ---

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: Option<f64>,
    pub eta_secs: Option<u64>,
    /// 0-based index of the file currently downloading (merges are 2 parts).
    pub part: u32,
    pub total_parts: u32,
    /// Merging / extracting audio after the raw download.
    pub processing: bool,
    /// Second attempt via the android player client after a 403/format error.
    pub retrying: bool,
}

#[derive(Clone, Debug)]
pub struct YoinkOutcome {
    pub out_path: PathBuf,
    pub title: String,
}

const PROGRESS_PREFIX: &str = "KONVRT|";
const PROGRESS_TEMPLATE: &str = "KONVRT|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s";

/// Full yt-dlp argument list for one download; pure so it's unit-testable.
pub fn build_download_args(
    url: &str,
    info_json: Option<&Path>,
    choice: &DownloadChoice,
    ffmpeg: Option<&Path>,
    out_dir: &Path,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut args = match info_json {
        Some(p) => vec![s("--load-info-json"), p.to_string_lossy().into_owned()],
        None => vec![s(url)],
    };
    args.extend(choice.format_args.iter().cloned());
    args.extend([
        s("--no-playlist"),
        s("--no-warnings"),
        s("--newline"),
        // --print implies --quiet, which suppresses the progress lines and
        // the [Merger]/[ExtractAudio] lines we detect the processing phase
        // from.
        s("--no-quiet"),
        s("--progress"),
        s("--progress-template"),
        format!("download:{PROGRESS_TEMPLATE}"),
        s("--print"),
        s("after_move:filepath"),
        s("--no-simulate"),
        s("-o"),
        out_dir
            .join("%(title).60s.%(ext)s")
            .to_string_lossy()
            .into_owned(),
    ]);
    if let Some(ffmpeg) = ffmpeg {
        args.extend([
            s("--ffmpeg-location"),
            ffmpeg.to_string_lossy().into_owned(),
        ]);
    }
    args
}

/// Errors where YouTube's SABR enforcement blocks the default player client;
/// a fresh extraction via the android client usually gets through.
pub fn should_fallback(error: &str) -> bool {
    error.contains("403")
        || error.contains("Forbidden")
        || error.contains("Requested format is not available")
}

/// Args for the fallback attempt: fresh extraction (the probe's info json was
/// extracted by the default client, so its stream URLs are useless here) plus
/// the android player client. Non-YouTube extractors ignore `youtube:*` args.
pub fn build_fallback_args(
    url: &str,
    choice: &DownloadChoice,
    ffmpeg: Option<&Path>,
    out_dir: &Path,
) -> Vec<String> {
    let mut args = build_download_args(url, None, choice, ffmpeg, out_dir);
    args.extend([
        "--extractor-args".to_string(),
        "youtube:player_client=android".to_string(),
    ]);
    args
}

fn to_number(value: &str) -> Option<f64> {
    if value.is_empty() || value == "NA" || value == "None" {
        return None;
    }
    let n: f64 = value.parse().ok()?;
    n.is_finite().then_some(n)
}

/// Parse one `KONVRT|downloaded|total|total_estimate|speed|eta` line.
/// Returns (downloaded, total, speed, eta).
/// (downloaded, total, speed, eta) from one progress-template line.
type ProgressLine = (u64, Option<u64>, Option<f64>, Option<u64>);

fn parse_progress_line(line: &str) -> Option<ProgressLine> {
    let rest = line.strip_prefix(PROGRESS_PREFIX)?;
    let mut fields = rest.split('|');
    let downloaded = to_number(fields.next()?).unwrap_or(0.0) as u64;
    let total = to_number(fields.next()?);
    let total_estimate = to_number(fields.next()?);
    let speed = to_number(fields.next()?);
    let eta = to_number(fields.next()?);
    Some((
        downloaded,
        total.or(total_estimate).map(|v| v as u64),
        speed,
        eta.map(|v| v as u64),
    ))
}

/// Port of cleanYtDlpError: last stderr line starting with "ERROR:", with the
/// "ERROR:" and any "[extractor]" prefix stripped.
fn clean_error(stderr: &str) -> String {
    let last = stderr
        .lines()
        .map(str::trim)
        .rfind(|l| l.starts_with("ERROR:"));
    let Some(last) = last else {
        return String::new();
    };
    let mut rest = last.strip_prefix("ERROR:").unwrap_or(last).trim_start();
    if rest.starts_with('[')
        && let Some(end) = rest.find(']')
    {
        rest = rest[end + 1..].trim_start();
    }
    rest.to_string()
}

/// Blocking download; live state goes into `progress`. A 403/format error on
/// the first attempt retries once via the android player client.
pub fn download(
    ytdlp: &Path,
    ffmpeg: Option<&Path>,
    url: &str,
    info_json: Option<&Path>,
    choice: &DownloadChoice,
    out_dir: &Path,
    progress: &Arc<Mutex<Progress>>,
) -> Result<YoinkOutcome> {
    let out_path = match run_attempt(
        ytdlp,
        &build_download_args(url, info_json, choice, ffmpeg, out_dir),
        progress,
        false,
    ) {
        Ok(path) => path,
        Err(e) if should_fallback(&format!("{e:#}")) => run_attempt(
            ytdlp,
            &build_fallback_args(url, choice, ffmpeg, out_dir),
            progress,
            true,
        )?,
        Err(e) => return Err(e),
    };
    let title = out_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| url.to_string());
    Ok(YoinkOutcome { out_path, title })
}

/// One yt-dlp invocation; returns the final file path printed by
/// `--print after_move:filepath`.
fn run_attempt(
    ytdlp: &Path,
    args: &[String],
    progress: &Arc<Mutex<Progress>>,
    retrying: bool,
) -> Result<PathBuf> {
    *progress.lock().unwrap() = Progress {
        total_parts: 1,
        retrying,
        ..Progress::default()
    };
    let mut child = Command::new(ytdlp)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("launching {}", ytdlp.display()))?;

    // Drain stderr on its own thread so a chatty yt-dlp can't fill the pipe
    // and deadlock against our stdout reads.
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = String::new();
        stderr.read_to_string(&mut buf).ok();
        buf
    });

    let mut filepath: Option<PathBuf> = None;
    let mut part: u32 = 0;
    let mut total_parts: u32 = 1;
    let mut last_downloaded: u64 = 0;
    let stdout = child.stdout.take().expect("stdout piped");
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((downloaded, total, speed, eta)) = parse_progress_line(line) {
            if downloaded < last_downloaded {
                part += 1;
            }
            last_downloaded = downloaded;
            *progress.lock().unwrap() = Progress {
                downloaded,
                total,
                speed,
                eta_secs: eta,
                part,
                total_parts,
                processing: false,
                retrying,
            };
        } else if line.contains("Downloading 1 format(s):") {
            // "[info] xxx: Downloading 1 format(s): 395+251" — each id is one
            // file yt-dlp fetches.
            total_parts = line
                .split("format(s):")
                .nth(1)
                .map(|ids| ids.trim().split('+').count() as u32)
                .unwrap_or(1)
                .max(1);
            progress.lock().unwrap().total_parts = total_parts;
        } else if line.contains("[Merger]") || line.contains("[ExtractAudio]") {
            progress.lock().unwrap().processing = true;
        } else if Path::new(line).is_absolute() {
            filepath = Some(PathBuf::from(line));
        }
    }

    let status = child.wait().context("waiting for yt-dlp")?;
    let stderr_text = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        let detail = clean_error(&stderr_text);
        if detail.is_empty() {
            bail!("download failed (yt-dlp exit {status})");
        }
        bail!("{detail}");
    }
    filepath.context("yt-dlp reported no output file")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    fn choice() -> DownloadChoice {
        DownloadChoice {
            label: "1080p · mp4".into(),
            audio: false,
            format_args: vec![
                "-f".into(),
                "bv*[height=1080]+ba/b[height=1080]/bv*[height<=1080]+ba/b".into(),
                "--merge-output-format".into(),
                "mp4".into(),
            ],
        }
    }

    #[test]
    fn download_args_with_info_json_and_ffmpeg() {
        let a = build_download_args(
            "https://youtu.be/x",
            Some(Path::new("/tmp/info.json")),
            &choice(),
            Some(Path::new("/opt/ffmpeg")),
            Path::new("/home/u/Downloads"),
        );
        assert_eq!(a[0], "--load-info-json");
        assert_eq!(a[1], "/tmp/info.json");
        assert!(!a.contains(&"https://youtu.be/x".to_string()));
        assert!(has_pair(
            &a,
            "-f",
            "bv*[height=1080]+ba/b[height=1080]/bv*[height<=1080]+ba/b"
        ));
        assert!(has_pair(&a, "--merge-output-format", "mp4"));
        assert!(a.contains(&"--no-quiet".to_string()));
        assert!(a.contains(&"--progress".to_string()));
        assert!(has_pair(
            &a,
            "--progress-template",
            "download:KONVRT|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s"
        ));
        assert!(has_pair(&a, "--print", "after_move:filepath"));
        assert!(a.contains(&"--no-simulate".to_string()));
        // Built through Path so the separator matches the host's.
        let expected_out = Path::new("/home/u/Downloads")
            .join("%(title).60s.%(ext)s")
            .to_string_lossy()
            .into_owned();
        assert!(has_pair(&a, "-o", &expected_out));
        assert!(has_pair(&a, "--ffmpeg-location", "/opt/ffmpeg"));
    }

    #[test]
    fn download_args_url_positional_without_info_json() {
        let a = build_download_args("https://youtu.be/x", None, &choice(), None, Path::new("/d"));
        assert_eq!(a[0], "https://youtu.be/x");
        assert!(!a.contains(&"--load-info-json".to_string()));
        assert!(!a.contains(&"--ffmpeg-location".to_string()));
    }

    #[test]
    fn builds_choices_from_formats() {
        let info = json!({
            "title": "t",
            "formats": [
                // audio-only: best is the 128k one
                {"format_id": "a1", "acodec": "opus", "vcodec": "none", "abr": 64.0, "filesize": 1_000_000},
                {"format_id": "a2", "acodec": "mp4a", "vcodec": "none", "abr": 128.0, "filesize": 2_000_000},
                // 1080p: webm has higher tbr but mp4 wins on the +10000 bonus
                {"format_id": "v1", "vcodec": "vp9", "acodec": "none", "ext": "webm", "height": 1080, "tbr": 5000.0, "filesize": 90_000_000},
                {"format_id": "v2", "vcodec": "avc1.64", "acodec": "none", "ext": "mp4", "height": 1080, "tbr": 4000.0, "filesize": 80_000_000},
                // 720p muxed: audio size must NOT be added
                {"format_id": "v3", "vcodec": "avc1", "acodec": "mp4a", "ext": "mp4", "height": 720, "tbr": 2000.0, "filesize_approx": 50_000_000},
            ]
        });
        let choices = build_choices(&info);
        assert_eq!(choices.len(), 3);

        // Heights descending, audio last.
        assert!(!choices[0].audio);
        // 80 MB video + 2 MB audio (mp4 candidate won, not muxed)
        assert_eq!(
            choices[0].label,
            format!("1080p · mp4 · ~{}", format_bytes(82_000_000.0))
        );
        assert_eq!(
            choices[0].format_args,
            vec![
                "-f",
                "bv*[height=1080]+ba/b[height=1080]/bv*[height<=1080]+ba/b",
                "--merge-output-format",
                "mp4"
            ]
        );
        // Muxed 720p: no audio size added.
        assert_eq!(
            choices[1].label,
            format!("720p · mp4 · ~{}", format_bytes(50_000_000.0))
        );

        let audio = &choices[2];
        assert!(audio.audio);
        assert_eq!(
            audio.label,
            format!("audio only · mp3 · ~{}", format_bytes(2_000_000.0))
        );
        assert_eq!(
            audio.format_args,
            vec![
                "-f",
                "ba/b",
                "-x",
                "--audio-format",
                "mp3",
                "--audio-quality",
                "0"
            ]
        );
    }

    #[test]
    fn choices_fallback_when_no_heights() {
        let info = json!({"title": "t", "formats": [
            {"format_id": "a", "acodec": "mp4a", "vcodec": "none"}
        ]});
        let choices = build_choices(&info);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].label, "best available · mp4");
        assert_eq!(
            choices[0].format_args,
            vec!["-f", "bv*+ba/b", "--merge-output-format", "mp4"]
        );
        assert!(choices[1].audio);
        assert_eq!(choices[1].label, "audio only · mp3"); // no size known

        // No formats at all still yields fallback + audio.
        let empty = build_choices(&json!({"title": "t"}));
        assert_eq!(empty.len(), 2);
    }

    #[test]
    fn caps_video_choices_at_eight() {
        let formats: Vec<Value> = (1..=12)
            .map(|i| json!({"format_id": i.to_string(), "vcodec": "avc1", "acodec": "none", "height": i * 100, "tbr": 100.0}))
            .collect();
        let choices = build_choices(&json!({"formats": formats}));
        assert_eq!(choices.len(), 9); // 8 video + audio
        assert_eq!(choices[0].label.split(' ').next(), Some("1200p"));
        assert_eq!(choices[7].label.split(' ').next(), Some("500p"));
    }

    #[test]
    fn parses_progress_template_lines() {
        assert_eq!(
            parse_progress_line("KONVRT|1000|2000|NA|512.5|10"),
            Some((1000, Some(2000), Some(512.5), Some(10)))
        );
        // total falls back to the estimate
        assert_eq!(
            parse_progress_line("KONVRT|500|NA|3000|NA|None"),
            Some((500, Some(3000), None, None))
        );
        assert_eq!(
            parse_progress_line("KONVRT|NA|NA|NA|NA|NA"),
            Some((0, None, None, None))
        );
        assert_eq!(parse_progress_line("[download] 45% of 10MiB"), None);
    }

    #[test]
    fn cleans_ytdlp_errors() {
        assert_eq!(
            clean_error("WARNING: x\nERROR: [youtube] abc: Sign in to confirm you're not a bot\n"),
            "abc: Sign in to confirm you're not a bot"
        );
        assert_eq!(
            clean_error("ERROR: first\nERROR: unable to download video data"),
            "unable to download video data"
        );
        assert_eq!(clean_error("just noise\n"), "");
    }

    #[test]
    fn formats_bytes_like_yoinks() {
        assert_eq!(format_bytes(0.0), "");
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(2_000_000.0), "1.9 MB");
        assert_eq!(format_bytes(82_000_000.0), "78 MB");
        assert_eq!(format_bytes(3.5 * 1024.0 * 1024.0 * 1024.0), "3.5 GB");
    }

    #[test]
    fn fallback_triggers_on_sabr_errors() {
        assert!(should_fallback("HTTP Error 403: Forbidden"));
        assert!(should_fallback("unable to download video data: Forbidden"));
        assert!(should_fallback(
            "Requested format is not available. Use --list-formats"
        ));
        assert!(!should_fallback("Sign in to confirm you're not a bot"));
        assert!(!should_fallback("Video unavailable"));
    }

    #[test]
    fn fallback_args_drop_info_json_and_add_android_client() {
        let a = build_fallback_args(
            "https://youtu.be/x",
            &choice(),
            Some(Path::new("/opt/ffmpeg")),
            Path::new("/d"),
        );
        assert_eq!(a[0], "https://youtu.be/x"); // fresh extraction: url positional
        assert!(!a.contains(&"--load-info-json".to_string()));
        assert!(has_pair(
            &a,
            "--extractor-args",
            "youtube:player_client=android"
        ));
        assert!(has_pair(&a, "--merge-output-format", "mp4"));
        assert!(has_pair(&a, "--ffmpeg-location", "/opt/ffmpeg"));
    }

    #[test]
    fn detects_probable_urls() {
        assert!(is_probable_url("https://www.youtube.com/watch?v=abc"));
        assert!(is_probable_url("http://x.com/user/status/1"));
        assert!(is_probable_url("  https://youtu.be/abc  "));
        assert!(!is_probable_url("youtube.com/watch?v=abc")); // no scheme
        assert!(!is_probable_url("https://localhost/video")); // no dot
        assert!(!is_probable_url("https://.com/x"));
        assert!(!is_probable_url("ftp://a.com/x"));
        assert!(!is_probable_url("https://a.com/some path"));
        assert!(!is_probable_url(""));
    }
}
