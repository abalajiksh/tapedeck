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

/// The pictures a story's frames may draw: each `href` in the story mapped to
/// a small PNG already cropped and sized for it.
pub type Images = HashMap<String, Arc<Vec<u8>>>;

/// The largest picture, per side, that will be decoded at all. A cover is a
/// few hundred pixels across; this is only a ceiling on what a hostile or
/// absurd file can make the decoder allocate (4096² is 64 MB of RGBA).
const MAX_SIDE: u32 = 4096;

/// What kind of picture these bytes are and how big, read from the header
/// alone. Only JPEG and PNG: they are what covers and portraits are in
/// practice, and each states its size where it can be read without decoding.
fn sniff(bytes: &[u8]) -> Option<(bool, u32, u32)> {
    if bytes.len() > 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((true, w, h));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // Walk the segments to the frame header, which carries the size.
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                return None;
            }
            let marker = bytes[i + 1];
            if marker == 0xFF {
                i += 1;
                continue;
            }
            let frame = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
            if frame {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((false, w, h));
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if len < 2 {
                return None;
            }
            i += 2 + len;
        }
    }
    None
}

fn kind(bytes: Arc<Vec<u8>>) -> Option<usvg::ImageKind> {
    match sniff(&bytes)? {
        (_, w, h) if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE => None,
        (true, ..) => Some(usvg::ImageKind::PNG(bytes)),
        (false, ..) => Some(usvg::ImageKind::JPEG(bytes)),
    }
}

/// usvg options that resolve an `<image>` from `images` and from nowhere
/// else.
///
/// **The default resolver reads the href as a path on this machine.** A story
/// puts addresses from the database into `href`s, so with the default a
/// stored value naming a local file would be read and painted into
/// somebody's video. Here an href either is one of the prepared pictures or
/// draws nothing, and a `data:` URL draws nothing either.
fn options(fontdb: &Arc<fontdb::Database>, images: &Arc<Images>) -> usvg::Options<'static> {
    let images = images.clone();
    let mut opt = usvg::Options {
        fontdb: fontdb.clone(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(move |href, _| images.get(href).cloned().and_then(kind)),
        },
        ..Default::default()
    };
    opt.font_family = crate::report_image::fonts::BODY.to_string();
    opt
}

/// Turn a fetched picture into what a frame draws: cropped to a square that
/// fills `side` pixels, as a PNG.
///
/// Done once per picture so that a frame decodes a few hundred pixels rather
/// than a 1200-pixel cover thirty times a second. `None` for anything that
/// is not a JPEG or PNG of a sane size, or that does not decode — the
/// lettered tile underneath is then what the video shows.
pub fn prepare(bytes: Vec<u8>, side: u32) -> Option<Vec<u8>> {
    let side = side.clamp(16, 1024);
    let mut one = Images::new();
    one.insert("p".into(), Arc::new(bytes));
    let fontdb = Arc::new(fontdb::Database::new());
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><image href="p" width="{side}" height="{side}" preserveAspectRatio="xMidYMid slice"/></svg>"#
    );
    let tree = usvg::Tree::from_str(&svg, &options(&fontdb, &Arc::new(one))).ok()?;
    let mut px = tiny_skia::Pixmap::new(side, side)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut px.as_mut());
    // A file that claimed to be a picture and did not decode leaves the
    // pixmap untouched; an empty square is not a cover.
    if px.pixels().iter().all(|p| p.alpha() == 0) {
        return None;
    }
    px.encode_png().ok()
}

fn draw(
    svg: &str,
    fontdb: &Arc<fontdb::Database>,
    images: &Arc<Images>,
    w: u32,
    h: u32,
) -> Result<tiny_skia::Pixmap, String> {
    let opt = options(fontdb, images);
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
pub fn encode(
    story: &Story,
    images: &Arc<Images>,
    out: &Path,
    done: &AtomicU32,
) -> Result<u64, String> {
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
                        fresh.then(|| sc.spawn(move || draw(svg, fontdb, images, w, h)))
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
    pub fn start(&self, user_id: i64, stem: String, story: Story, images: Images) -> Job {
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
                encode(&story, &Arc::new(images), &path, &done)
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

    /// A picture is prepared to the size it is drawn at, and nothing that is
    /// not a sane JPEG or PNG is decoded at all.
    #[test]
    fn a_picture_is_sized_once_and_a_bad_one_is_refused_unread() {
        let mut src = tiny_skia::Pixmap::new(300, 200).unwrap();
        src.fill(tiny_skia::Color::from_rgba8(200, 60, 40, 255));
        let png = src.encode_png().unwrap();
        assert_eq!(sniff(&png), Some((true, 300, 200)));

        let small = prepare(png.clone(), 148).expect("a real PNG is prepared");
        let out = tiny_skia::Pixmap::decode_png(&small).unwrap();
        assert_eq!((out.width(), out.height()), (148, 148), "cropped to fill the square");
        let mid = out.pixel(74, 74).unwrap();
        assert!(mid.red() > 150 && mid.alpha() == 255, "and it is the picture, not a blank");

        // A PNG header that claims a size no cover has is never handed on.
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        assert!(kind(Arc::new(huge.clone())).is_none());
        assert!(prepare(huge, 148).is_none());

        for bad in [
            &b"GIF89a......"[..],
            b"<svg xmlns='http://www.w3.org/2000/svg'/>",
            b"",
            b"\xFF\xD8\xFF",
        ] {
            assert!(prepare(bad.to_vec(), 148).is_none());
        }
        // A baseline JPEG header: SOI, an APP0 segment, then SOF0 800x600.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02,
            0x58, 0x03, 0x20, 0x03, 0x01, 0x22, 0x00,
        ];
        assert_eq!(sniff(&jpeg), Some((false, 800, 600)));
    }

    /// A frame draws a prepared picture and nothing else an `href` names: not
    /// a file on this machine, and not a `data:` URL.
    #[test]
    fn a_frame_draws_prepared_pictures_and_never_a_local_file() {
        let fontdb = Arc::new(fontdb::Database::new());
        let mut src = tiny_skia::Pixmap::new(64, 64).unwrap();
        src.fill(tiny_skia::Color::from_rgba8(10, 200, 10, 255));
        let png = src.encode_png().unwrap();
        let path = std::env::temp_dir().join(format!("tapedeck-href-{}.png", std::process::id()));
        std::fs::write(&path, &png).unwrap();

        let frame = |href: &str, images: &Arc<Images>| {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><image href="{href}" width="64" height="64"/></svg>"#
            );
            let px = draw(&svg, &fontdb, images, 64, 64).expect("draw");
            px.pixels().iter().any(|p| p.alpha() > 0)
        };
        let mut held = Images::new();
        held.insert("/api/v1/art/5".into(), Arc::new(png));
        let held = Arc::new(held);
        assert!(frame("/api/v1/art/5", &held), "a prepared picture is drawn");
        assert!(!frame(path.to_str().unwrap(), &held), "a path on this machine is not read");
        assert!(!frame("/api/v1/art/6", &held), "an href nobody prepared draws nothing");
        let _ = std::fs::remove_file(&path);
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
        let story =
            Story::build(&Card::sample(Unit::Year), Unit::Year, Theme::Dark, &Default::default());
        let out = Path::new(&dir).join("story-year.mp4");
        let done = AtomicU32::new(0);
        let began = Instant::now();
        let bytes = encode(&story, &Default::default(), &out, &done).expect("encode");
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
