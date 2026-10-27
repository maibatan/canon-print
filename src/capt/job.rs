//! One-page LBP2900 print job — the captdriver sequence (prn_lbp2900.c, commit 62719249)
//! that printed on our unit (jobs 60-72), with two deliberate changes:
//! 1. every wait has a timeout and reports its stage + last status;
//! 2. page-counter waits read A0A8 on every poll, so the counters are real
//!    (captdriver only reads A0A8 when STATUS0 bit 8 is set and otherwise uses stale or
//!    uninitialised values — docs/protocol.md §4.3). canon-LBP2900-for-macOS does the
//!    same and prints on real hardware.
//!
//! All fixed waits keep captdriver's 1 s: shortening them made the printer hang or
//! ignore FIRE in our hardware tests (jobs 53, 65, 66, 73).

use super::commands::{self, Gpio, JobStage, JobTime, PageParams};
use super::session::Session;
use super::status::{BasicStatus, ExtendedStatus, PageCounters};
use crate::compression::hiscoa::{self, Params};
use crate::error::{CanonError, Result};
use crate::transport::UsbTransport;
use serde::Serialize;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// wait after IDENT (captdriver sleep(1))
    pub ident_wait: Duration,
    /// interval between status polls (captdriver sleep(1))
    pub poll: Duration,
    /// wait before the first "page received" check after PRINT_DATA_END (captdriver sleep(1))
    pub data_end_wait: Duration,
    /// limit for BUSY / BUFFERFULL waits
    pub ready_timeout: Duration,
    /// limit for "page received" / "page out" / "job completed" waits
    pub page_timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            ident_wait: Duration::from_secs(1),
            poll: Duration::from_secs(1),
            data_end_wait: Duration::from_secs(1),
            ready_timeout: Duration::from_secs(30),
            page_timeout: Duration::from_secs(90),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct StageRecord {
    pub stage: String,
    pub status: String,
    pub page: Option<usize>,
    pub elapsed_ms: u128,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct JobReport {
    pub stages: Vec<StageRecord>,
    pub job_id: Option<u16>,
    pub initialized_printer: bool,
    pub print_data_packets: usize,
    pub print_data_bytes: usize,
    pub final_counters: Option<PageCounters>,
    pub failed_stage: Option<String>,
    pub failed_page: Option<usize>,
    pub pages_printed: usize,
}

/// Wall-clock time for JOB_SETUP. captdriver uses localtime(); this uses UTC
/// (the printer shows no use of it; INFERRED harmless).
pub fn job_time_now() -> JobTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    JobTime {
        year_since_1900: (year - 1900) as u16,
        month0: month - 1,
        day,
        hour: (rem / 3600) as u8,
        minute: (rem % 3600 / 60) as u8,
        second: (rem % 60) as u8,
    }
}

pub struct PrintJob<'a, T: UsbTransport> {
    pub s: &'a mut Session<T>,
    pub timing: Timing,
    pub report: JobReport,
    started: Instant,
    last_counters: PageCounters,
    last_status: String,
    /// stage currently running (reported as the failing stage on any error)
    current: &'static str,
    page_no: Option<usize>,
}

fn timeout(stage: &str, since: Instant, last: &str) -> CanonError {
    CanonError::Timeout {
        stage: stage.to_string(),
        elapsed: since.elapsed(),
        last_status: last.to_string(),
    }
}

impl<'a, T: UsbTransport> PrintJob<'a, T> {
    pub fn new(s: &'a mut Session<T>, timing: Timing) -> Self {
        PrintJob {
            s,
            timing,
            report: JobReport::default(),
            started: Instant::now(),
            last_counters: PageCounters {
                decoding: 0,
                printing: 0,
                out: 0,
                completed: 0,
                received: 0,
            },
            last_status: "none".into(),
            current: "identify",
            page_no: None,
        }
    }

    fn mark(&mut self, stage: &str, status: &str) {
        self.report.stages.push(StageRecord {
            stage: stage.to_string(),
            status: status.to_string(),
            page: self.page_no,
            elapsed_ms: self.started.elapsed().as_millis(),
        });
    }

    fn sleep(d: Duration) {
        if !d.is_zero() {
            std::thread::sleep(d);
        }
    }

    fn basic(&mut self) -> Result<BasicStatus> {
        let b = self.s.basic_status()?;
        self.last_status = format!("STATUS0=0x{:04X}", b.status0);
        Ok(b)
    }

    /// E0A0 + A0A8 (always), remembering the counters.
    fn xstatus(&mut self) -> Result<(BasicStatus, ExtendedStatus)> {
        let b = self.basic()?;
        let x = self.s.extended_status()?;
        self.last_counters = x.pages;
        self.last_status = format!(
            "STATUS0=0x{:04X} XSTATUS0=0x{:04X} STATUS1=0x{:04X} pages dec/prt/out/cmp/rcv={}/{}/{}/{}/{}",
            b.status0,
            x.status[0],
            x.status[1],
            x.pages.decoding,
            x.pages.printing,
            x.pages.out,
            x.pages.completed,
            x.pages.received
        );
        Ok((b, x))
    }

    /// captdriver capt_wait_ready: poll E0A0 while BUSY.
    fn wait_ready(&mut self, stage: &str) -> Result<()> {
        let since = Instant::now();
        loop {
            if !self.basic()?.busy() {
                return Ok(());
            }
            if since.elapsed() >= self.timing.ready_timeout {
                return Err(timeout(
                    &format!("{stage}: waiting for BUSY to clear"),
                    since,
                    &self.last_status,
                ));
            }
            Self::sleep(self.timing.poll);
        }
    }

    /// Whole job: `pages` = encoded bands of each sheet, printed in order in one job
    /// (captdriver prints every page of a CUPS job inside one JOB_BEGIN/JOB_END). Copies
    /// are passed as repeated references to the same encoded page, not as clones.
    pub fn run(&mut self, page: &PageParams, params: &Params, pages: &[&[Vec<u8>]]) -> Result<()> {
        self.started = Instant::now();
        let result = if pages.is_empty() {
            Err(CanonError::Input("no page to print".into()))
        } else {
            self.run_inner(page, params, pages)
        };
        if result.is_err() {
            self.report.failed_stage = Some(self.current.to_string());
            self.report.failed_page = self.page_no;
            self.mark(self.current, "FAIL");
            if let Some(job) = self.report.job_id {
                self.cleanup(job);
            }
        }
        self.report.final_counters = Some(self.last_counters);
        result
    }

    fn run_inner(
        &mut self,
        page: &PageParams,
        params: &Params,
        pages: &[&[Vec<u8>]],
    ) -> Result<()> {
        let t = job_time_now();

        // job prologue (captdriver lbp2900_job_prologue)
        self.current = "identify";
        self.s.identify()?;
        self.mark("identify", "PASS");
        self.current = "status";
        Self::sleep(self.timing.ident_wait);
        self.xstatus()?;
        self.mark("status", "PASS");
        self.current = "reserve";
        self.s.sendrecv(&commands::start_0())?;
        let r = self.s.sendrecv(&commands::job_begin())?;
        if r.payload.len() < 4 {
            return Err(CanonError::Protocol(format!(
                "JOB_BEGIN reply payload has {} bytes, need 4 for the job id",
                r.payload.len()
            )));
        }
        let job = u16::from_le_bytes([r.payload[2], r.payload[3]]);
        self.report.job_id = Some(job);
        self.s.sendrecv(&commands::gpio(Gpio::Lbp3010Init))?;
        self.wait_ready("reserve")?;
        self.mark("reserve", "PASS");
        self.current = "start_job";
        self.s
            .sendrecv(&commands::job_setup(JobStage::Start, 0, job, t))?;
        self.wait_ready("start_job")?;
        self.mark("start_job", "PASS");

        let mut decoding = 0u16;
        for (i, bands) in pages.iter().enumerate() {
            self.page_no = Some(i + 1);
            decoding = self.print_page(page, params, bands, job, t)?;
            self.report.pages_printed = i + 1;
        }
        self.page_no = None;

        // job epilogue (lbp2900_job_epilogue)
        self.current = "end_job";
        let since = Instant::now();
        loop {
            let (_, x) = self.xstatus()?;
            if x.pages.completed == decoding {
                self.s.sendrecv(&commands::job_setup(
                    JobStage::Finished,
                    x.pages.completed,
                    job,
                    t,
                ))?;
                break;
            }
            if since.elapsed() >= self.timing.page_timeout {
                return Err(timeout(
                    "end_job: waiting for the last page to complete",
                    since,
                    &self.last_status,
                ));
            }
            Self::sleep(self.timing.poll);
        }
        self.s.sendrecv(&commands::job_end(job))?;
        self.mark("end_job", "PASS");
        self.mark("completion", "PASS");
        Ok(())
    }

    /// One page: page prologue, bands, page epilogue. Returns the printer's page number.
    fn print_page(
        &mut self,
        page: &PageParams,
        params: &Params,
        bands: &[Vec<u8>],
        job: u16,
        t: JobTime,
    ) -> Result<u16> {
        // page prologue (lbp2900_page_prologue)
        self.current = "start_page";
        let (b, _) = self.xstatus()?;
        if b.uninitialized() {
            self.s.sendrecv(&commands::start_1())?;
            self.s.sendrecv(&commands::start_2())?;
            self.s.sendrecv(&commands::start_3())?;
            self.wait_ready("start_page")?;
            self.s.sendrecv(&commands::upload_2())?;
            self.wait_ready("start_page")?;
            self.report.initialized_printer = true;
        }
        let since = Instant::now();
        while self.basic()?.buffer_full() {
            if since.elapsed() >= self.timing.ready_timeout {
                return Err(timeout(
                    "start_page: waiting for BUFFERFULL to clear",
                    since,
                    &self.last_status,
                ));
            }
            Self::sleep(self.timing.poll);
        }
        self.s.send(&commands::set_parms(page, params)?)?;
        self.mark("start_page", "PASS");

        // bands (ops_send_band_hiscoa): wait_ready before every 16th packet of the page
        self.current = "transfer";
        let mut isend = 0usize;
        for band in bands {
            for pkt in commands::print_data(band) {
                isend += 1;
                if isend % 16 == 0 {
                    self.wait_ready("transfer")?;
                }
                self.report.print_data_bytes += pkt.payload.len();
                self.s.send(&pkt)?;
            }
        }
        self.report.print_data_packets += isend;
        self.mark("transfer", "PASS");

        // page epilogue (lbp2900_page_epilogue)
        self.current = "end_page";
        self.s.send(&commands::print_data_end())?;
        let since = Instant::now();
        let decoding = loop {
            Self::sleep(self.timing.data_end_wait);
            let (_, x) = self.xstatus()?;
            if x.pages.received == x.pages.decoding {
                break x.pages.decoding;
            }
            if since.elapsed() >= self.timing.page_timeout {
                return Err(timeout(
                    "end_page: waiting for the printer to receive the page",
                    since,
                    &self.last_status,
                ));
            }
        };
        self.s.sendrecv(&commands::job_setup(
            JobStage::PageComplete,
            decoding,
            job,
            t,
        ))?;
        self.wait_ready("end_page")?;
        self.s.sendrecv(&commands::fire(decoding))?;
        self.wait_ready("end_page")?;
        self.s
            .sendrecv(&commands::job_setup(JobStage::Fired, decoding, job, t))?;
        let since = Instant::now();
        loop {
            let (_, x) = self.xstatus()?;
            if x.pages.out == decoding {
                break;
            }
            if x.no_paper() && !x.printing() {
                return Err(CanonError::Protocol(format!(
                    "end_page: printer out of paper ({})",
                    self.last_status
                )));
            }
            if since.elapsed() >= self.timing.page_timeout {
                return Err(timeout(
                    "end_page: waiting for the page to come out",
                    since,
                    &self.last_status,
                ));
            }
            Self::sleep(self.timing.poll);
        }
        self.mark("end_page", "PASS");
        Ok(decoding)
    }

    /// Best effort, like captdriver lbp2900_cancel_cleanup: never leave the printer
    /// reserved by a half-finished job.
    fn cleanup(&mut self, job: u16) {
        let saved = self.s.timeout;
        self.s.timeout = Duration::from_secs(2);
        let t = job_time_now();
        let _ = self.s.sendrecv(&commands::gpio(Gpio::Lbp2900Init));
        let _ = self.s.sendrecv(&commands::job_setup(
            JobStage::Finished,
            self.last_counters.completed,
            job,
            t,
        ));
        let _ = self.s.sendrecv(&commands::job_end(job));
        self.s.timeout = saved;
    }
}

/// Encode a page raster into bands (convenience for callers).
pub fn encode(raster: &[u8], page: &PageParams, params: &Params) -> Result<Vec<Vec<u8>>> {
    hiscoa::encode_page(
        raster,
        page.line_size as usize,
        page.num_lines as usize,
        params,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capt::commands::op;
    use crate::capt::lbp2900::Paper;
    use crate::compression::hiscoa::DEFAULT_PARAMS;
    use crate::transport::FakePrinter;

    fn fast() -> Timing {
        Timing {
            ident_wait: Duration::ZERO,
            poll: Duration::ZERO,
            data_end_wait: Duration::ZERO,
            ready_timeout: Duration::from_millis(300),
            page_timeout: Duration::from_millis(300),
        }
    }

    fn bands() -> (PageParams, Vec<Vec<u8>>) {
        let page = Paper::A4.page_params();
        let bmp = crate::fixtures::find("text_test_600x600")
            .unwrap()
            .generate();
        let raster = bmp.to_page_raster(page.line_size as usize, page.num_lines as usize);
        let b = encode(&raster, &page, &DEFAULT_PARAMS).unwrap();
        (page, b)
    }

    fn run(printer: FakePrinter) -> (Result<()>, JobReport, FakePrinter) {
        let (page, b) = bands();
        let mut s = Session::new(printer, Duration::from_millis(100));
        let mut job = PrintJob::new(&mut s, fast());
        let r = job.run(&page, &DEFAULT_PARAMS, &[&b]);
        let report = job.report.clone();
        (r, report, s.transport)
    }

    #[test]
    fn happy_path_follows_captdriver_sequence() {
        let (r, report, p) = run(FakePrinter::default());
        r.unwrap();
        let stages: Vec<_> = report.stages.iter().map(|s| s.stage.as_str()).collect();
        assert_eq!(
            stages,
            [
                "identify",
                "status",
                "reserve",
                "start_job",
                "start_page",
                "transfer",
                "end_page",
                "end_job",
                "completion"
            ]
        );
        assert!(
            report.initialized_printer,
            "UNINIT printer must get START_1..3 + UPLOAD_2"
        );
        assert_eq!(report.job_id, Some(0x0042));
        assert_eq!(report.print_data_packets, 97);
        assert_eq!(p.data_bytes, report.print_data_bytes);
        // order of the key commands
        let pos = |c: u16| p.log.iter().position(|&x| x == c).unwrap();
        assert!(pos(op::IDENT) < pos(op::JOB_BEGIN));
        assert!(pos(op::JOB_BEGIN) < pos(op::UPLOAD_2));
        assert!(pos(op::UPLOAD_2) < pos(op::SET_PARMS));
        assert!(pos(op::SET_PARMS) < pos(op::PRINT_DATA));
        assert!(pos(op::PRINT_DATA_END) < pos(op::FIRE));
        assert_eq!(*p.log.last().unwrap(), op::JOB_END);
        let c = report.final_counters.unwrap();
        assert_eq!((c.decoding, c.out, c.completed), (1, 1, 1));
    }

    #[test]
    fn three_pages_in_one_job() {
        let (page, b) = bands();
        let mut s = Session::new(FakePrinter::default(), Duration::from_millis(100));
        let mut job = PrintJob::new(&mut s, fast());
        job.run(&page, &DEFAULT_PARAMS, &[&b, &b, &b]).unwrap();
        let report = job.report.clone();
        let p = s.transport;
        assert_eq!(report.pages_printed, 3);
        assert_eq!(report.print_data_packets, 3 * 97);
        let count = |c: u16| p.log.iter().filter(|&&x| x == c).count();
        assert_eq!(count(op::JOB_BEGIN), 1);
        assert_eq!(count(op::JOB_END), 1);
        assert_eq!(count(op::FIRE), 3);
        assert_eq!(count(op::SET_PARMS), 3);
        assert_eq!(count(op::UPLOAD_2), 1, "init only once");
        assert_eq!(report.final_counters.unwrap().completed, 3);
        assert!(report
            .stages
            .iter()
            .any(|s| s.stage == "end_page" && s.page == Some(3)));
    }

    #[test]
    fn empty_job_is_an_input_error() {
        let page = Paper::A4.page_params();
        let mut s = Session::new(FakePrinter::default(), Duration::from_millis(100));
        let mut job = PrintJob::new(&mut s, fast());
        assert_eq!(
            job.run(&page, &DEFAULT_PARAMS, &[])
                .unwrap_err()
                .exit_code(),
            1
        );
        assert!(
            s.transport.log.is_empty(),
            "nothing may be sent for an empty job"
        );
    }

    #[test]
    fn initialized_printer_skips_init_sequence() {
        let mut p = FakePrinter::default();
        p.initialized = true;
        let (r, report, p) = run(p);
        r.unwrap();
        assert!(!report.initialized_printer);
        assert!(!p.log.contains(&op::UPLOAD_2));
    }

    #[test]
    fn ignored_fire_times_out_at_end_page_and_releases_the_job() {
        let mut p = FakePrinter::default();
        p.ignore_fire = true;
        let (r, report, p) = run(p);
        let e = r.unwrap_err();
        assert_eq!(e.exit_code(), 4, "{e}");
        assert_eq!(report.failed_stage.as_deref(), Some("end_page"));
        assert_eq!(report.failed_page, Some(1));
        assert!(e.to_string().contains("page to come out"), "{e}");
        assert_eq!(
            *p.log.last().unwrap(),
            op::JOB_END,
            "cleanup must release the job"
        );
    }

    #[test]
    fn no_paper_is_reported() {
        let mut p = FakePrinter::default();
        p.no_paper = true;
        p.initialized = true;
        let (r, report, p) = run(p);
        let e = r.unwrap_err();
        assert!(e.to_string().contains("out of paper"), "{e}");
        assert_eq!(report.failed_stage.as_deref(), Some("end_page"));
        assert_eq!(*p.log.last().unwrap(), op::JOB_END);
    }

    #[test]
    fn job_time_is_sane() {
        let t = job_time_now();
        assert!(t.year_since_1900 >= 125 && t.month0 < 12 && (1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 60);
    }
}
