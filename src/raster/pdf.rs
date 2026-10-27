//! PDF → 1-bit page rasters, rendered by Ghostscript at runtime (optional dependency:
//! only needed for PDF input; PBM input needs nothing).
//!
//! The raster size is the imageable area CUPS/Ghostscript produced for the LBP2900 PPD
//! on our VM (scripts/cups-raster-params.py: A4 4722 × 6780, A5 3264 × 4722 at 600 dpi,
//! 14 pt hardware margins), and pages are scaled to fit (`-dPDFFitPage`, like the
//! `fit-to-page` option HHH-Bridge passed to CUPS). The result is then centred into the
//! page line exactly like captdriver (Bitmap::to_page_raster).
//!
//! Ghostscript renders 8-bit gray (`pgmraw`) and canon-print halftones it with
//! Ghostscript's own default screen and transfer (raster::halftone): same result as
//! Ghostscript's 1-bit output on gs 9.55, but independent of the Ghostscript version
//! (gs ≥ 9.56 lightens photos on pages with transparency, see halftone.rs).

use super::{halftone, pbm, pgm, Bitmap};
use crate::capt::lbp2900::Paper;
use crate::error::{CanonError, Result};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// A complex page can take Ghostscript a while, but never this long without writing a
/// byte: then it is stuck. canon-print stops it and reports stage "render" (nothing was
/// sent to the printer, so HHH-Bridge can fall back to CUPS).
pub const RENDER_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Ghostscript text kept in messages and warnings (its decisive line comes last).
const GS_TEXT_LIMIT: usize = 4000;

/// Where `%PDF-` may appear: PDF readers (and Ghostscript) accept junk before it.
const PDF_HEADER_SEARCH: usize = 1024;

/// Pages of a document, plus anything Ghostscript complained about while rendering it
/// successfully (e.g. "page not found" in a damaged PDF: fewer pages than expected).
#[derive(Debug)]
pub struct Loaded {
    pub pages: Vec<Bitmap>,
    pub warnings: Option<String>,
}

/// Rendered raster size per paper: (width, height) in dots at 600 dpi.
pub fn render_size(paper: Paper) -> (usize, usize) {
    match paper {
        Paper::A4 => (4722, 6780),
        Paper::A5 => (3264, 4722),
    }
}

pub fn is_pdf(head: &[u8]) -> bool {
    let head = &head[..head.len().min(PDF_HEADER_SEARCH)];
    head.windows(5).any(|w| w == b"%PDF-")
}

/// Ghostscript arguments for an 8-bit gray render of every page at the paper's size.
pub fn gs_args(paper: Paper) -> Vec<String> {
    let (w, h) = render_size(paper);
    [
        "-q",
        "-dSAFER",
        "-dBATCH",
        "-dNOPAUSE",
        "-sDEVICE=pgmraw",
        "-r600",
        &format!("-g{w}x{h}"),
        "-dFIXEDMEDIA",
        "-dPDFFitPage",
        // Ghostscript prints its error messages on stdout: keep stdout for image data only
        "-sstdout=%stderr",
        "-sOutputFile=-",
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Render every page of `path` (8-bit gray from Ghostscript, halftoned here page by page).
pub fn render(path: &Path, paper: Paper) -> Result<Loaded> {
    render_with("gs", path, paper, RENDER_IDLE_TIMEOUT)
}

/// Last `GS_TEXT_LIMIT` bytes of Ghostscript's output as text (invalid UTF-8 replaced:
/// Ghostscript echoes raw PDF bytes in some error messages).
fn gs_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.len() <= GS_TEXT_LIMIT {
        return text.to_string();
    }
    let mut cut = text.len() - GS_TEXT_LIMIT;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    format!("… ({cut} bytes omitted) {}", &text[cut..])
}

fn with_text(msg: String, text: &str) -> String {
    if text.is_empty() {
        msg
    } else {
        format!("{msg}: {text}")
    }
}

/// Message of a raster error without the "raster error:" prefix of its Display.
fn inner(e: CanonError) -> String {
    match e {
        CanonError::Raster(m) => m,
        other => other.to_string(),
    }
}

/// Stop Ghostscript and everything it started (it runs in its own process group).
fn kill(child: &Mutex<Child>) {
    let mut c = child.lock().unwrap_or_else(|p| p.into_inner());
    #[cfg(unix)]
    unsafe {
        // the group id is the child's pid (process_group(0)). The child is not reaped yet,
        // or was reaped a moment ago on this thread (try_wait in render_with): Linux hands
        // out pids cyclically, so the id cannot have been reused in between
        libc::kill(-(c.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = c.kill();
}

#[cfg(unix)]
fn killed_by_sigkill(status: &ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn killed_by_sigkill(_status: &ExitStatus) -> bool {
    false
}

/// Counts bytes coming from Ghostscript so the watchdog can tell "slow" from "stuck".
struct Activity<R> {
    inner: R,
    start: Instant,
    last_ms: Arc<AtomicU64>,
}

impl<R: Read> Read for Activity<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.last_ms
                .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
        }
        Ok(n)
    }
}

fn render_with(program: &str, path: &Path, paper: Paper, idle: Duration) -> Result<Loaded> {
    let (w, h) = render_size(paper);
    let mut cmd = Command::new(program);
    cmd.args(gs_args(paper))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // own process group: stopping it also stops children of a gs wrapper script
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        CanonError::Permission(format!(
            "cannot run Ghostscript ('{program}') to render the PDF: {e} (sudo apt install ghostscript)"
        ))
    })?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let child = Arc::new(Mutex::new(child));

    // stderr is drained on its own thread (a full pipe would block Ghostscript) and handed
    // over through a channel, so a process that keeps the pipe open cannot block us
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        let _ = err_tx.send(buf);
    });

    // watchdog: stop Ghostscript when it writes nothing for `idle`
    let start = Instant::now();
    let last_ms = Arc::new(AtomicU64::new(0));
    let timed_out = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let watchdog = {
        let (child, last_ms, timed_out) = (
            Arc::clone(&child),
            Arc::clone(&last_ms),
            Arc::clone(&timed_out),
        );
        std::thread::spawn(move || loop {
            match done_rx.recv_timeout(Duration::from_millis(250)) {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let quiet = (start.elapsed().as_millis() as u64)
                        .saturating_sub(last_ms.load(Ordering::Relaxed));
                    if quiet >= idle.as_millis() as u64 {
                        timed_out.store(true, Ordering::Relaxed);
                        kill(&child);
                        return;
                    }
                }
                _ => return, // render finished (or the sender was dropped)
            }
        })
    };

    let mut out = BufReader::with_capacity(
        1 << 20,
        Activity {
            inner: stdout,
            start,
            last_ms,
        },
    );
    let mut pages = Vec::new();
    let mut gray = Vec::new();
    let parsed = loop {
        let header = match pgm::read_header(&mut out) {
            Ok(Some(size)) => size,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        if header != (w, h) {
            break Err(CanonError::Raster(format!(
                "page {} rendered as {}x{}, expected {w}x{h}",
                pages.len() + 1,
                header.0,
                header.1
            )));
        }
        if let Err(e) = pgm::read_body(&mut out, w, h, &mut gray) {
            break Err(e);
        }
        pages.push(halftone::gs_screen(&gray, w, h));
    };
    drop(done_tx);
    let _ = watchdog.join();
    let timed_out = timed_out.load(Ordering::Relaxed);

    // on a parse error: had Ghostscript already failed by itself (crash, out of memory)?
    // Then stop it (pages nobody will read) and anything it left holding our pipes.
    let mut stopped_by_us = timed_out;
    if parsed.is_err() && !timed_out {
        let already = child
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_wait()
            .ok()
            .flatten();
        kill(&child);
        stopped_by_us = already.is_none();
    }
    drop(out);
    let status = child
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .wait()
        .map_err(|e| CanonError::Raster(format!("waiting for Ghostscript: {e}")))?;
    let text = gs_text(
        &err_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default(),
    );

    if timed_out {
        return Err(CanonError::Timeout {
            stage: "render".into(),
            elapsed: idle,
            last_status: with_text(
                format!(
                    "Ghostscript wrote nothing for {} s after {} page(s)",
                    idle.as_secs(),
                    pages.len()
                ),
                &text,
            ),
        });
    }
    if let Err(e) = parsed {
        // our own SIGKILL is not a Ghostscript failure; any other unsuccessful end is
        let ended_by_us = stopped_by_us && killed_by_sigkill(&status);
        let gs_failed = !(status.success() || ended_by_us);
        let msg = if gs_failed {
            with_text(
                format!("Ghostscript failed ({status}) [{}]", inner(e)),
                &text,
            )
        } else {
            with_text(inner(e), &text)
        };
        return Err(CanonError::Raster(msg));
    }
    if !status.success() {
        return Err(CanonError::Raster(with_text(
            format!("Ghostscript failed ({status})"),
            &text,
        )));
    }
    if pages.is_empty() {
        return Err(CanonError::Raster(with_text(
            "Ghostscript produced no page".into(),
            &text,
        )));
    }
    Ok(Loaded {
        pages,
        warnings: (!text.is_empty()).then_some(text),
    })
}

/// Load an input file: PDF (via Ghostscript) or PBM (one or more P4 images).
pub fn load_pages(path: &Path, paper: Paper) -> Result<Loaded> {
    let cannot =
        |e: std::io::Error| CanonError::Input(format!("cannot read {}: {e}", path.display()));
    let mut head = Vec::with_capacity(PDF_HEADER_SEARCH);
    std::fs::File::open(path)
        .and_then(|f| f.take(PDF_HEADER_SEARCH as u64).read_to_end(&mut head))
        .map_err(cannot)?;
    if head.is_empty() {
        return Err(CanonError::Input(format!("{} is empty", path.display())));
    }
    // a PBM's pixel bytes could contain "%PDF-" by chance: its magic decides first
    let pbm_magic = head.starts_with(b"P4") || head.starts_with(b"P1");
    if !pbm_magic && is_pdf(&head) {
        return render(path, paper);
    }
    let data = std::fs::read(path).map_err(cannot)?;
    Ok(Loaded {
        pages: pbm::parse_all(&data)?,
        warnings: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gs_available() -> bool {
        Command::new("gs")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn renders_two_page_pdf_at_the_cups_size() {
        if !gs_available() {
            eprintln!("SKIPPED: Ghostscript not installed");
            return;
        }
        let pages = render(Path::new("tests/fixtures/two_pages.pdf"), Paper::A4)
            .unwrap()
            .pages;
        assert_eq!(pages.len(), 2);
        for p in &pages {
            assert_eq!((p.width, p.height), (4722, 6780));
            assert!(
                p.black_pixel_count() > 1000,
                "text must render black pixels"
            );
        }
        let a5 = render(Path::new("tests/fixtures/two_pages.pdf"), Paper::A5)
            .unwrap()
            .pages;
        assert_eq!((a5[0].width, a5[0].height), (3264, 4722));
    }

    fn ramp_rows(b: &Bitmap) -> &[u8] {
        // the ramp is drawn near the top of the page, the soft-masked image near the bottom
        &b.data[..3000 * b.stride]
    }

    #[test]
    fn tone_does_not_depend_on_transparency() {
        // Ghostscript >= 9.56 halftoned the ramp of tone_ramp_smask.pdf ~13 % lighter than
        // that of tone_ramp.pdf (its transfer was applied twice on pages with transparency).
        if !gs_available() {
            eprintln!("SKIPPED: Ghostscript not installed");
            return;
        }
        let plain = render(Path::new("tests/fixtures/tone_ramp.pdf"), Paper::A4)
            .unwrap()
            .pages;
        let masked = render(Path::new("tests/fixtures/tone_ramp_smask.pdf"), Paper::A4)
            .unwrap()
            .pages;
        assert!(
            ramp_rows(&plain[0]) == ramp_rows(&masked[0]),
            "ramp must print identically"
        );
        assert!(
            plain[0].black_pixel_count() > 100_000,
            "ramp must be rendered"
        );
    }

    #[test]
    fn matches_ghostscript_halftone_on_pages_without_transparency() {
        // gs pbmraw applies its transfer exactly once on pages without transparency (every
        // version): that is the reference canon-print reproduces on every page.
        if !gs_available() {
            eprintln!("SKIPPED: Ghostscript not installed");
            return;
        }
        let fixture = Path::new("tests/fixtures/tone_ramp.pdf");
        let ours = render(fixture, Paper::A4).unwrap().pages.remove(0);
        let args: Vec<String> = gs_args(Paper::A4)
            .into_iter()
            .map(|a| {
                if a == "-sDEVICE=pgmraw" {
                    "-sDEVICE=pbmraw".to_string()
                } else {
                    a
                }
            })
            .collect();
        let out = Command::new("gs")
            .args(&args)
            .arg(fixture)
            .output()
            .unwrap();
        assert!(out.status.success());
        let theirs = pbm::parse(&out.stdout).unwrap();
        assert_eq!((theirs.width, theirs.height), (ours.width, ours.height));
        let differing: u32 = ours
            .data
            .iter()
            .zip(&theirs.data)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        let fraction = differing as f64 / (ours.width * ours.height) as f64;
        // measured 0.013 % (rounding inside Ghostscript's image halftoning)
        assert!(
            fraction < 0.001,
            "{:.4} % of the dots differ",
            fraction * 100.0
        );
    }

    #[test]
    fn broken_pdf_reports_ghostscript_error() {
        if !gs_available() {
            eprintln!("SKIPPED: Ghostscript not installed");
            return;
        }
        let path =
            std::env::temp_dir().join(format!("canon-print-broken-{}.pdf", std::process::id()));
        std::fs::write(&path, b"%PDF-1.4\nthis is not a pdf").unwrap();
        let err = render(&path, Paper::A4).unwrap_err().to_string();
        let _ = std::fs::remove_file(&path);
        // Ghostscript's own words, not just canon-print's wording around them
        assert!(err.contains("Error") || err.contains("error"), "{err}");
        assert!(
            !err.contains("PGM"),
            "Ghostscript's text must not be parsed as image data: {err}"
        );
    }

    #[test]
    fn detects_pdf_and_loads_pbm() {
        assert!(is_pdf(b"%PDF-1.7\n"));
        assert!(
            is_pdf(b"\n\n%PDF-1.4\n"),
            "junk before the header is allowed"
        );
        assert!(!is_pdf(b"P4\n1 1\n\x00"));
        let loaded = load_pages(Path::new("tests/fixtures/black_8x8.pbm"), Paper::A4).unwrap();
        assert_eq!(loaded.pages.len(), 1);
        assert!(loaded.warnings.is_none());
        let empty =
            std::env::temp_dir().join(format!("canon-print-empty-{}.pdf", std::process::id()));
        std::fs::write(&empty, b"").unwrap();
        let err = load_pages(&empty, Paper::A4).unwrap_err();
        let _ = std::fs::remove_file(&empty);
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn gs_text_keeps_the_end_and_replaces_invalid_utf8() {
        assert_eq!(
            gs_text(b"  Error: /undefined in b\xe1o.pdf \n"),
            "Error: /undefined in b\u{fffd}o.pdf"
        );
        let long = vec![b'x'; GS_TEXT_LIMIT + 500];
        let t = gs_text(&[&long[..], b"LAST LINE"].concat());
        assert!(
            t.starts_with("… (509 bytes omitted)") && t.ends_with("LAST LINE"),
            "{t}"
        );
    }

    /// Fake Ghostscript: a shell script that ignores its arguments.
    #[cfg(unix)]
    fn fake_gs(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("canon-print-fakegs-{}-{name}", std::process::id()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn run_fake(name: &str, body: &str, idle: Duration) -> (Result<Loaded>, Duration) {
        let gs = fake_gs(name, body);
        let t = Instant::now();
        let r = render_with(gs.to_str().unwrap(), Path::new("x.pdf"), Paper::A5, idle);
        let _ = std::fs::remove_file(&gs);
        (r, t.elapsed())
    }

    #[cfg(unix)]
    #[test]
    fn wrong_size_header_fails_before_reading_the_body() {
        // a valid header for the wrong size, then nothing: must not wait for the body
        let (r, t) = run_fake(
            "size",
            "printf 'P5\\n20000 20000\\n255\\n'; sleep 30",
            RENDER_IDLE_TIMEOUT,
        );
        let e = r.unwrap_err().to_string();
        assert!(e.contains("expected 3264x4722"), "{e}");
        assert!(t < Duration::from_secs(5), "took {t:?}");
    }

    #[cfg(unix)]
    #[test]
    fn stuck_ghostscript_times_out_as_render_stage() {
        let (r, t) = run_fake("stuck", "sleep 30", Duration::from_millis(500));
        match r.unwrap_err() {
            CanonError::Timeout { stage, .. } => assert_eq!(stage, "render"),
            other => panic!("expected a timeout, got {other}"),
        }
        assert!(t < Duration::from_secs(5), "took {t:?}");
    }

    #[cfg(unix)]
    #[test]
    fn stuck_wrapper_child_is_stopped_too() {
        // a wrapper whose background child keeps stdout open after the wrapper exits
        let (r, t) = run_fake(
            "wrapper",
            "sleep 30 &\nprintf 'P5\\n1 1\\n255\\n\\000'",
            RENDER_IDLE_TIMEOUT,
        );
        assert!(r.unwrap_err().to_string().contains("expected 3264x4722"));
        assert!(t < Duration::from_secs(5), "took {t:?}");
    }

    #[cfg(unix)]
    #[test]
    fn ghostscript_crash_is_reported_as_such() {
        let (r, _) = run_fake(
            "crash",
            "printf 'P5\\n3264 4722\\n255\\nxx'; kill -SEGV $$",
            RENDER_IDLE_TIMEOUT,
        );
        let e = r.unwrap_err().to_string();
        assert!(
            e.contains("Ghostscript failed") && e.contains("truncated"),
            "{e}"
        );
        assert!(!e.contains("raster error: raster error"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_stderr_is_kept() {
        let (r, _) = run_fake(
            "latin1",
            "printf 'Error: /undefinedfilename in (b\\341o.pdf)' >&2; exit 1",
            RENDER_IDLE_TIMEOUT,
        );
        let e = r.unwrap_err().to_string();
        assert!(
            e.contains("undefinedfilename") && e.contains('\u{fffd}'),
            "{e}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn warnings_of_a_successful_render_are_returned() {
        let body = "echo '**** Error: page 2 not found' >&2\nhead -c 15412608 /dev/zero | { printf 'P5\\n3264 4722\\n255\\n'; cat; }";
        let (r, _) = run_fake("warn", body, RENDER_IDLE_TIMEOUT);
        let loaded = r.unwrap();
        assert_eq!(loaded.pages.len(), 1);
        assert!(loaded.warnings.unwrap().contains("page 2 not found"));
    }
}
