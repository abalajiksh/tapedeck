//! A report story as an MP4: the frames drawn here, the encoding done by
//! `ffmpeg`.
//!
//! **ffmpeg is optional and found at run time.** Tapedeck stays one binary
//! with no C in it; a host that has `ffmpeg` with libx264 on its `PATH` (or at
//! `TAPEDECK_FFMPEG`) can export a video, and one that does not still plays
//! the story in the browser and simply does not offer the export.
//!
//! What the Pi 4B measurement (2026-11-10) decided:
//!
//! * A frame is **redrawn from SVG**, not composited — 62 ms against 93 ms.
//! * **Drawing is the bottleneck, not encoding.** x264 `veryfast` took every
//!   frame as fast as it was offered, so the effort here goes into drawing
//!   less: a frame whose SVG is byte-identical to the one before is written
//!   again without being rasterised, and a story is mostly held frames.
//! * The Pi's hardware encoder was no faster and made files three times the
//!   size, so there is one encoder and no probing for another.
//!
//! One render runs at a time on the whole instance, on two drawing threads at
//! most with ffmpeg capped to two of its own, so a household Pi keeps cores
//! for the engine tick while somebody exports their year.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::report_image::story::Story;

/// How long a finished video is kept before it is deleted.
const KEEP: Duration = Duration::from_secs(60 * 60);
/// Finished videos kept per user; the oldest goes first.
const KEEP_PER_USER: usize = 3;

fn ffmpeg_bin() -> String {
    std::env::var("TAPEDECK_FFMPEG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("ffmpeg".into())
}

/// Whether this host can encode: an `ffmpeg` that runs and has libx264.
///
/// Asked once and remembered. **Blocking** — it runs a process — so call it
/// from `spawn_blocking`. Installing ffmpeg later needs a restart to be
/// noticed, which is the price of not spawning a process per page view.
pub fn available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let found = Command::new(ffmpeg_bin())
            .args(["-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("libx264"))
            .unwrap_or(false);
        if found {
            tracing::info!("🎞️ ffmpeg with libx264 found: report stories can be exported as video");
        } else {
            tracing::info!("🎞️ No ffmpeg with libx264: report stories play in the browser only");
        }
        found
    })
}

fn draw(
    svg: &str,
    fontdb: &Arc<fontdb::Database>,
    w: u32,
    h: u32,
) -> Result<tiny_skia::Pixmap, String> {
    let mut opt = usvg::Options { fontdb: fontdb.clone(), ..Default::default() };
    opt.font_family = crate::report_image::fonts::BODY.to_string();
    let tree =
        usvg::Tree::from_str(svg, &opt).map_err(|e| format!("a frame is not valid SVG: {e}"))?;
    let mut px = tiny_skia::Pixmap::new(w, h).ok_or("could not allocate a frame")?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut px.as_mut());
    Ok(px)
}

/// Draw every frame of `story` and encode it to `out`. Returns the file size.
///
/// **Blocking and long** (tens of seconds on a Pi): run it on
/// `spawn_blocking`. `done` is advanced as frames are handed to the encoder.
pub fn encode(story: &Story, out: &Path, done: &AtomicU32) -> Result<u64, String> {
    let (w, h, fps) = (story.width, story.height, story.fps);
    let total = story.frames();
    if total == 0 {
        return Err("the story has no frames".into());
    }

    let mut child = Command::new(ffmpeg_bin())
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pixel_format", "rgba"])
        .args(["-video_size", &format!("{w}x{h}"), "-framerate", &fps.to_string(), "-i", "pipe:0"])
        .args(["-an", "-c:v", "libx264", "-preset", "veryfast", "-tune", "animation", "-crf", "20"])
        .args(["-threads", "2", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-f", "mp4"])
        .arg(out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg has no stdin")?;

    let fontdb = Arc::new(crate::report_image::fonts::db().clone());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .saturating_sub(2)
        .clamp(1, 2);

    let fed = (|| -> Result<(), String> {
        let mut prev_svg = String::new();
        let mut prev_px: Vec<u8> = Vec::new();
        let mut f = 0u32;
        while f < total {
            let n = (workers as u32).min(total - f);
            let svgs: Vec<String> = (f..f + n)
                .map(|i| {
                    let ms = (i as u64 * 1000 / fps as u64) as u32;
                    let (scene, t) = story.locate(ms);
                    story.frame_svg(scene, t)
                })
                .collect();
            // A frame identical to the one before it is not drawn again.
            let fresh: Vec<bool> = svgs
                .iter()
                .enumerate()
                .map(|(k, s)| *s != if k == 0 { prev_svg.as_str() } else { svgs[k - 1].as_str() })
                .collect();

            let drawn: Vec<Option<Result<tiny_skia::Pixmap, String>>> = std::thread::scope(|sc| {
                let handles: Vec<_> = svgs
                    .iter()
                    .zip(&fresh)
                    .map(|(svg, fresh)| {
                        let fontdb = &fontdb;
                        fresh.then(|| sc.spawn(move || draw(svg, fontdb, w, h)))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.map(|h| h.join().unwrap_or_else(|_| Err("a frame panicked".into()))))
                    .collect()
            });

            for px in drawn {
                if let Some(px) = px {
                    prev_px = px?.take();
                }
                stdin.write_all(&prev_px).map_err(|e| format!("ffmpeg stopped reading: {e}"))?;
                done.fetch_add(1, Ordering::Relaxed);
            }
            prev_svg = svgs.into_iter().next_back().unwrap_or_default();
            f += n;
        }
        Ok(())
    })();
    drop(stdin);

    let mut stderr = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let status = child.wait().map_err(|e| format!("ffmpeg did not finish: {e}"))?;
    fed?;
    if !status.success() {
        let tail: String = stderr.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        return Err(format!("ffmpeg failed ({status}): {tail}"));
    }
    std::fs::metadata(out).map(|m| m.len()).map_err(|e| format!("no video was written: {e}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Rendering,
    Done,
    Failed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Rendering => "rendering",
            Phase::Done => "done",
            Phase::Failed => "failed",
        }
    }
}

/// One export, as its owner sees it.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub user_id: i64,
    /// A filename stem: the period, reduced to letters, digits and dashes.
    pub stem: String,
    pub phase: Phase,
    pub frames_total: u32,
    pub frames_done: Arc<AtomicU32>,
    pub path: PathBuf,
    pub bytes: u64,
    /// Why it failed, in words fit to show.
    pub error: Option<String>,
    started: Instant,
}

/// Every export this instance knows about. In memory on purpose: a video is
/// regenerable from the history in under two minutes, so a restart losing the
/// list loses nothing a second click does not bring back.
#[derive(Clone)]
pub struct VideoJobs {
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    /// One render at a time on the whole instance.
    gate: Arc<tokio::sync::Semaphore>,
    dir: PathBuf,
}

impl Default for VideoJobs {
    fn default() -> Self {
        VideoJobs {
            jobs: Default::default(),
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            // The temp dir, not the uploads dir: uploads are user data and
            // these are not. Per process, so two instances on one host (the
            // Pi and a test binary) cannot collect each other's files.
            dir: std::env::temp_dir().join(format!("tapedeck-video-{}", std::process::id())),
        }
    }
}

impl VideoJobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The caller's job, or `None` — also for somebody else's id, which is
    /// indistinguishable from one that never existed.
    pub fn get(&self, id: &str, user_id: i64) -> Option<Job> {
        self.lock().get(id).filter(|j| j.user_id == user_id).cloned()
    }

    /// The export this user already has in flight, if any.
    pub fn active(&self, user_id: i64) -> Option<Job> {
        self.lock()
            .values()
            .find(|j| j.user_id == user_id && matches!(j.phase, Phase::Queued | Phase::Rendering))
            .cloned()
    }

    /// Forget finished exports that are old, or beyond what one user keeps,
    /// and delete their files.
    fn prune(&self, user_id: i64) {
        let mut jobs = self.lock();
        let mut gone: Vec<String> = jobs
            .values()
            .filter(|j| {
                matches!(j.phase, Phase::Done | Phase::Failed) && j.started.elapsed() > KEEP
            })
            .map(|j| j.id.clone())
            .collect();
        let mut mine: Vec<&Job> = jobs
            .values()
            .filter(|j| j.user_id == user_id && matches!(j.phase, Phase::Done | Phase::Failed))
            .collect();
        mine.sort_by_key(|j| std::cmp::Reverse(j.started));
        gone.extend(mine.iter().skip(KEEP_PER_USER - 1).map(|j| j.id.clone()));
        for id in gone {
            if let Some(j) = jobs.remove(&id) {
                let _ = std::fs::remove_file(&j.path);
            }
        }
    }

    /// Queue an export and start it in the background. Returns at once.
    pub fn start(&self, user_id: i64, stem: String, story: Story) -> Job {
        self.prune(user_id);
        let id = {
            use rand::Rng;
            let mut rng = rand::rng();
            (0..16).map(|_| format!("{:02x}", rng.random::<u8>())).collect::<String>()
        };
        let job = Job {
            id: id.clone(),
            user_id,
            stem,
            phase: Phase::Queued,
            frames_total: story.frames(),
            frames_done: Arc::new(AtomicU32::new(0)),
            path: self.dir.join(format!("{id}.mp4")),
            bytes: 0,
            error: None,
            started: Instant::now(),
        };
        self.lock().insert(id.clone(), job.clone());

        let this = self.clone();
        let (path, done) = (job.path.clone(), job.frames_done.clone());
        tokio::spawn(async move {
            let set = |f: &dyn Fn(&mut Job)| {
                if let Some(j) = this.lock().get_mut(&id) {
                    f(j);
                }
            };
            // Waiting here is the queue: one permit, taken in arrival order.
            let Ok(_permit) = this.gate.clone().acquire_owned().await else {
                return;
            };
            set(&|j| j.phase = Phase::Rendering);
            let began = Instant::now();
            let dir = this.dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&dir).map_err(|e| format!("no room for the video: {e}"))?;
                encode(&story, &path, &done)
            })
            .await
            .unwrap_or_else(|e| Err(format!("the render stopped: {e}")));
            match result {
                Ok(bytes) => {
                    tracing::info!(
                        "🎞️ Story video {id} for user {user_id}: {bytes} bytes in {:.1}s",
                        began.elapsed().as_secs_f32()
                    );
                    set(&|j| {
                        j.bytes = bytes;
                        j.phase = Phase::Done;
                    });
                }
                Err(e) => {
                    tracing::error!("Story video {id} for user {user_id} failed: {e}");
                    set(&|j| {
                        j.phase = Phase::Failed;
                        j.error = Some("The video could not be made".into());
                        let _ = std::fs::remove_file(&j.path);
                    });
                }
            }
        });
        job
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_image::{Card, Theme, Unit};

    /// Somebody else's job id answers exactly as an unknown one does.
    #[test]
    fn a_job_is_only_ever_its_owners() {
        let jobs = VideoJobs::default();
        let job = Job {
            id: "abc".into(),
            user_id: 7,
            stem: "2026".into(),
            phase: Phase::Rendering,
            frames_total: 10,
            frames_done: Default::default(),
            path: PathBuf::from("/nonexistent/abc.mp4"),
            bytes: 0,
            error: None,
            started: Instant::now(),
        };
        jobs.lock().insert("abc".into(), job);
        assert!(jobs.get("abc", 7).is_some());
        assert!(jobs.get("abc", 8).is_none());
        assert!(jobs.active(7).is_some());
        assert!(jobs.active(8).is_none());
    }

    /// Encodes the sample year for real and checks ffmpeg was fed every frame.
    /// Ignored: it needs ffmpeg and takes seconds.
    /// `TAPEDECK_CARD_DIR=/tmp/story cargo test -- --ignored encode_the_sample`
    #[test]
    #[ignore]
    fn encode_the_sample_year() {
        assert!(available(), "this needs an ffmpeg with libx264");
        let dir = std::env::var("TAPEDECK_CARD_DIR").expect("set TAPEDECK_CARD_DIR");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let story = Story::build(&Card::sample(Unit::Year), Unit::Year, Theme::Dark);
        let out = Path::new(&dir).join("story-year.mp4");
        let done = AtomicU32::new(0);
        let began = Instant::now();
        let bytes = encode(&story, &out, &done).expect("encode");
        assert_eq!(done.load(Ordering::Relaxed), story.frames());
        println!(
            "{} ({bytes} bytes, {} frames, {:.1}s of video in {:.1}s)",
            out.display(),
            story.frames(),
            story.duration_ms() as f32 / 1000.0,
            began.elapsed().as_secs_f32()
        );
    }
}
