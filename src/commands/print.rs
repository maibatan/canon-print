//! `canon-print print <file>`: staged pipeline with per-stage status.
//!
//! input (PDF via Ghostscript, or PBM with one or more pages) → page rasters
//! (captdriver-style centring into the LBP2900 page geometry) → Hi-SCoA → one job with
//! every page (× copies, collated). `--dry-run` stops before USB. "success" means the
//! printer reported every page completed; whether the paper looks right is never known
//! to the program ("physical_page": "NOT VERIFIED").

use super::Outcome;
use crate::capt::commands;
use crate::capt::job::{PrintJob, Timing};
use crate::capt::lbp2900::Paper;
use crate::capt::packet::hex;
use crate::capt::session::{Dumper, Session};
use crate::compression::hiscoa;
use crate::error::CanonError;
use crate::raster::pdf;
use crate::usb_transport::RusbTransport;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

pub const MAX_COPIES: u32 = 99;

pub struct PrintOptions<'a> {
    pub paper: &'a str,
    pub copies: u32,
    pub dry_run: bool,
    pub detach: bool,
    pub dump: Option<&'a Path>,
    pub verbose: bool,
}

fn fail(
    stages: Map<String, Value>,
    stage: &str,
    err: CanonError,
    page: Option<usize>,
    dry_run: bool,
) -> Outcome {
    Outcome {
        text: format!(
            "FAIL at: {stage}{}\n{err}\n",
            page.map(|p| format!(" (page {p})")).unwrap_or_default()
        ),
        exit_code: err.exit_code(),
        json: json!({
            "success": false,
            "stage": stage,
            "error": err.kind(),
            "message": err.to_string(),
            "page": page,
            "stages": stages,
            "dry_run": dry_run,
            "physical_page": "NOT VERIFIED",
        }),
    }
}

pub fn run(file: &Path, o: &PrintOptions) -> Outcome {
    let mut stages = Map::new();
    let input_fail = |mut stages: Map<String, Value>, e| {
        stages.insert("input".into(), json!("FAIL"));
        fail(stages, "input", e, None, o.dry_run)
    };

    let paper = match Paper::parse(o.paper) {
        Ok(p) => p,
        Err(e) => return input_fail(stages, e),
    };
    if o.copies == 0 || o.copies > MAX_COPIES {
        return input_fail(
            stages,
            CanonError::Input(format!("copies must be 1..={MAX_COPIES}")),
        );
    }
    let images = match pdf::load_pages(file, paper) {
        Ok(p) => p,
        Err(e) => {
            // Ghostscript missing is an environment problem, not bad input
            let stage = if matches!(e, CanonError::Permission(_)) {
                "render"
            } else {
                "input"
            };
            stages.insert(stage.into(), json!("FAIL"));
            return fail(stages, stage, e, None, o.dry_run);
        }
    };
    stages.insert("input".into(), json!("PASS"));

    let params = paper.page_params();
    let (line_size, lines) = (params.line_size as usize, params.num_lines as usize);
    let mut encoded: Vec<Vec<Vec<u8>>> = Vec::with_capacity(images.len());
    for (i, img) in images.iter().enumerate() {
        let raster = img.to_page_raster(line_size, lines);
        match hiscoa::encode_page(&raster, line_size, lines, &hiscoa::DEFAULT_PARAMS) {
            Ok(b) => encoded.push(b),
            Err(e) => {
                stages.insert("encode".into(), json!("FAIL"));
                return fail(stages, "encode", e, Some(i + 1), o.dry_run);
            }
        }
    }
    if let Err(e) = commands::set_parms(&params, &hiscoa::DEFAULT_PARAMS) {
        stages.insert("encode".into(), json!("FAIL"));
        return fail(stages, "encode", e, None, o.dry_run);
    }
    stages.insert("encode".into(), json!("PASS"));

    // collated copies: the whole document `copies` times, in one job
    let job_pages: Vec<Vec<Vec<u8>>> = (0..o.copies)
        .flat_map(|_| encoded.iter().cloned())
        .collect();
    let compressed: Vec<usize> = encoded
        .iter()
        .map(|b| b.iter().map(Vec::len).sum())
        .collect();
    let summary = json!({
        "paper": paper.name(),
        "document_pages": images.len(),
        "copies": o.copies,
        "sheets": job_pages.len(),
        "input_size": images.first().map(|i| [i.width, i.height]),
        "line_size": line_size,
        "lines": lines,
        "bands_per_page": lines.div_ceil(hiscoa::BAND_LINES),
        "compressed_bytes_per_page": compressed,
        "set_parm_page_hex": hex(&params.to_bytes()),
    });

    if o.dry_run {
        return Outcome {
            text: format!(
                "dry run: {} page(s) {} x {} copies = {} sheet(s); page {line_size} B x {lines} lines; \
                 Hi-SCoA bytes per page {:?} (USB not opened)\n",
                images.len(),
                paper.name(),
                o.copies,
                job_pages.len(),
                compressed
            ),
            exit_code: 0,
            json: json!({
                "success": true,
                "dry_run": true,
                "pages": job_pages.len(),
                "job": summary,
                "stages": stages,
                "physical_page": "NOT VERIFIED",
            }),
        };
    }

    // real printer
    let dumper = match o.dump.map(Dumper::create).transpose() {
        Ok(d) => d,
        Err(e) => return fail(stages, "setup", e, None, o.dry_run),
    };
    let transport = match RusbTransport::open(o.detach) {
        Ok(t) => {
            for s in ["detect", "open", "claim_interface"] {
                stages.insert(s.into(), json!("PASS"));
            }
            t
        }
        Err(e) => {
            let stage = match &e {
                CanonError::Usb(m) if m.contains("no Canon") => "detect",
                CanonError::Permission(m) if m.contains("claimed") || m.contains("busy") => {
                    "claim_interface"
                }
                _ => "open",
            };
            stages.insert(stage.into(), json!("FAIL"));
            return fail(stages, stage, e, None, o.dry_run);
        }
    };
    let mut session = Session::new(transport, Duration::from_secs(5))
        .with_dump(dumper)
        .verbose(o.verbose);
    // the reader thread has been listening since open(); a short look is enough
    let drained = session
        .drain_stale(Duration::from_millis(20), 50)
        .unwrap_or(0);
    let mut job = PrintJob::new(&mut session, Timing::default());
    let result = job.run(&params, &hiscoa::DEFAULT_PARAMS, &job_pages);
    let report = job.report.clone();
    for st in &report.stages {
        stages.insert(st.stage.clone(), json!(st.status));
    }
    match result {
        Ok(()) => Outcome {
            text: format!(
                "PRINT JOB COMPLETED: {} sheet(s) (printer reported them done; physical pages NOT VERIFIED)\n\
                 job id {:?}, {} PRINT_DATA packets, {} bytes, printer initialized by this job: {}\n",
                report.pages_printed,
                report.job_id,
                report.print_data_packets,
                report.print_data_bytes,
                report.initialized_printer
            ),
            exit_code: 0,
            json: json!({
                "success": true,
                "print_job": "COMPLETED",
                "physical_page": "NOT VERIFIED",
                "pages": report.pages_printed,
                "job": summary,
                "stages": stages,
                "report": report,
                "stale_bytes_drained": drained,
            }),
        },
        Err(e) => {
            let stage = report.failed_stage.clone().unwrap_or_else(|| "unknown".into());
            let mut out = fail(stages, &stage, e, report.failed_page, o.dry_run);
            if let Value::Object(m) = &mut out.json {
                m.insert("report".into(), json!(report));
                m.insert("pages_printed".into(), json!(report.pages_printed));
            }
            out
        }
    }
}
