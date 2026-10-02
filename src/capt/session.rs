//! CAPT request/reply session over a [`UsbTransport`].
//!
//! Replies are reassembled by their length field: the LBP2900 sends the first 6 bytes
//! as one transfer and the rest separately (captdriver SPECS:49-50), and our unit's
//! bulk IN endpoint has 512-byte packets (so captdriver's "multiple of 64" rule is not
//! used). A BCD length (captdriver SPECS:52-54) is accepted when a short transfer ends
//! the reply at exactly the BCD value. Every exchange has a deadline.

use super::commands::{self, op};
use super::packet::{bcd_len, hex, Packet};
use super::status::{BasicStatus, ExtendedStatus, PrinterInfo};
use crate::error::{CanonError, Result};
use crate::transport::UsbTransport;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Human name of an opcode, for logs.
pub fn op_name(code: u16) -> &'static str {
    match code {
        op::IDENT => "IDENT",
        op::CHKSTATUS => "CHKSTATUS",
        op::CHKXSTATUS => "CHKXSTATUS",
        op::START_0 => "START_0",
        op::JOB_BEGIN => "JOB_BEGIN",
        op::GPIO => "GPIO",
        op::JOB_SETUP => "JOB_SETUP",
        op::START_1 => "START_1",
        op::START_2 => "START_2",
        op::START_3 => "START_3",
        op::UPLOAD_2 => "UPLOAD_2",
        op::SET_PARMS => "SET_PARMS",
        op::PRINT_DATA => "PRINT_DATA",
        op::PRINT_DATA_END => "PRINT_DATA_END",
        op::FIRE => "FIRE",
        op::JOB_END => "JOB_END",
        _ => "UNKNOWN",
    }
}

/// Writes every TX/RX packet to `dir/tx-NNNN.bin`, `dir/rx-NNNN.bin` and a text log.
pub struct Dumper {
    dir: PathBuf,
    n: usize,
    start: Instant,
    log: std::fs::File,
}

impl Dumper {
    pub fn create(dir: &Path) -> Result<Dumper> {
        std::fs::create_dir_all(dir)
            .map_err(|e| CanonError::Input(format!("cannot create {}: {e}", dir.display())))?;
        let log = std::fs::File::create(dir.join("protocol.log"))
            .map_err(|e| CanonError::Input(format!("cannot create protocol.log: {e}")))?;
        Ok(Dumper {
            dir: dir.to_path_buf(),
            n: 0,
            start: Instant::now(),
            log,
        })
    }

    fn record(&mut self, dir: &str, data: &[u8], note: &str) {
        if dir == "tx" {
            self.n += 1;
        }
        let _ = std::fs::write(self.dir.join(format!("{dir}-{:04}.bin", self.n)), data);
        let shown = &data[..data.len().min(32)];
        let _ = writeln!(
            self.log,
            "{:10.3} {} {:04} {:<14} {} bytes: {}{}",
            self.start.elapsed().as_secs_f64(),
            dir.to_uppercase(),
            self.n,
            note,
            data.len(),
            hex(shown),
            if data.len() > 32 { " …" } else { "" }
        );
    }
}

pub struct Session<T: UsbTransport> {
    pub transport: T,
    pub timeout: Duration,
    dumper: Option<Dumper>,
    verbose: bool,
}

impl<T: UsbTransport> Session<T> {
    pub fn new(transport: T, timeout: Duration) -> Session<T> {
        Session {
            transport,
            timeout,
            dumper: None,
            verbose: false,
        }
    }

    pub fn with_dump(mut self, dumper: Option<Dumper>) -> Self {
        self.dumper = dumper;
        self
    }

    pub fn verbose(mut self, on: bool) -> Self {
        self.verbose = on;
        self
    }

    fn trace(&mut self, dir: &str, data: &[u8], note: &str) {
        if self.verbose {
            eprintln!(
                "{} {:<14} {}{}",
                dir.to_uppercase(),
                note,
                hex(&data[..data.len().min(32)]),
                if data.len() > 32 { " …" } else { "" }
            );
        }
        if let Some(d) = self.dumper.as_mut() {
            d.record(dir, data, note);
        }
    }

    pub fn send(&mut self, p: &Packet) -> Result<()> {
        let bytes = p.to_bytes();
        self.trace("tx", &bytes, op_name(p.opcode));
        self.transport.write(&bytes, self.timeout)
    }

    /// Receive one complete reply to `request`.
    pub fn recv(&mut self, request: u16) -> Result<Packet> {
        let start = Instant::now();
        let deadline = start + self.timeout;
        let mut data: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 64 * 1024]; // multiple of any bulk max packet size
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(self.timeout_error(request, start, &data));
            }
            let n = match self.transport.read(&mut buf, deadline - now) {
                Ok(n) => n,
                Err(CanonError::Timeout { .. }) => {
                    return Err(self.timeout_error(request, start, &data))
                }
                Err(e) => return Err(e),
            };
            data.extend_from_slice(&buf[..n]);
            if data.len() < 4 {
                continue;
            }
            let binary = u16::from_le_bytes([data[2], data[3]]) as usize;
            let short = n < self.transport.max_packet_size();
            let done = data.len() == binary
                || (short && data.len() >= 6 && bcd_len(data[2], data[3]) == Some(data.len()));
            if done || (binary >= 4 && data.len() > binary) {
                break; // too long is reported by Packet::parse below
            }
        }
        self.trace("rx", &data, op_name(request));
        Packet::parse_reply(&data, request)
    }

    fn timeout_error(&self, request: u16, start: Instant, got: &[u8]) -> CanonError {
        CanonError::Timeout {
            stage: format!("reply to {}", op_name(request)),
            elapsed: start.elapsed(),
            last_status: if got.is_empty() {
                "nothing received".into()
            } else {
                format!("partial reply {}", hex(got))
            },
        }
    }

    pub fn sendrecv(&mut self, p: &Packet) -> Result<Packet> {
        self.send(p)?;
        self.recv(p.opcode)
    }

    /// Discard bytes left in the IN pipe by an earlier job/session (the CAPT-Toolkit
    /// captd daemon found that a late reply desyncs the next job). Returns the count.
    pub fn drain_stale(&mut self, per_read: Duration, max_rounds: usize) -> Result<usize> {
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0;
        for _ in 0..max_rounds {
            match self.transport.read(&mut buf, per_read) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    self.trace("rx", &chunk, "STALE");
                    total += n;
                }
                Err(CanonError::Timeout { .. }) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(total)
    }

    pub fn identify(&mut self) -> Result<PrinterInfo> {
        let r = self.sendrecv(&commands::ident())?;
        PrinterInfo::parse(&r.payload)
    }

    pub fn basic_status(&mut self) -> Result<BasicStatus> {
        let r = self.sendrecv(&commands::chk_status())?;
        BasicStatus::parse(&r.payload)
    }

    pub fn extended_status(&mut self) -> Result<ExtendedStatus> {
        let r = self.sendrecv(&commands::chk_xstatus())?;
        ExtendedStatus::parse(&r.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    const A1A1: [u8; 56] = [
        0xA1, 0xA1, 0x38, 0x00, 0x00, 0x0B, 0x31, 0x2A, 0x01, 0x01, 0xF0, 0xFF, 0x40, 0x00, 0x04,
        0x00, 0x41, 0x00, 0x01, 0x00, 0xD0, 0x02, 0x00, 0x00, 0x6F, 0x08, 0x00, 0x00, 0xE4, 0x0D,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFA, 0x02, 0x00, 0x00, 0xF6, 0x04, 0x00, 0x00, 0x28,
        0x3C, 0x32, 0x32, 0x58, 0x02, 0x58, 0x02, 0x15, 0x03, 0x02, 0x02,
    ];
    const E0A0: [u8; 12] = [
        0xA0, 0xE0, 0x0C, 0x00, 0x00, 0x8A, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00,
    ];

    fn session(m: MockTransport) -> Session<MockTransport> {
        Session::new(m, Duration::from_millis(200))
    }

    #[test]
    fn identify_with_split_reply() {
        let mut s = session(MockTransport::new().reply_split(&A1A1));
        let info = s.identify().unwrap();
        assert_eq!(info.dpi, (600, 600));
        assert_eq!(s.transport.written, vec![vec![0xA1, 0xA1, 0x04, 0x00]]);
    }

    #[test]
    fn status_reply_from_hw_log() {
        let mut s = session(MockTransport::new().reply_split(&E0A0));
        assert_eq!(s.basic_status().unwrap().status0, 0x8A00);
    }

    #[test]
    fn whole_reply_in_one_transfer() {
        let mut s = session(MockTransport::new().reply(&E0A0));
        assert!(s.basic_status().is_ok());
    }

    #[test]
    fn bcd_length_reply() {
        let mut r = vec![0xA1, 0xA1, 0x57, 0x00];
        r.extend(A1A1[4..].iter());
        r.resize(57, 0); // decimal 57 bytes, 0x57 would be 87
        let mut s = session(MockTransport::new().reply(&r[..6]).reply(&r[6..]));
        assert!(s.identify().is_ok());
    }

    #[test]
    fn silent_printer_times_out_with_stage() {
        let mut s = session(MockTransport::new());
        let e = s.identify().unwrap_err();
        assert_eq!(e.exit_code(), 4);
        assert!(e.to_string().contains("reply to IDENT"), "{e}");
    }

    #[test]
    fn partial_reply_timeout_reports_bytes() {
        let mut s = session(MockTransport::new().reply(&A1A1[..6]));
        let e = s.identify().unwrap_err();
        assert!(
            e.to_string().contains("partial reply A1 A1 38 00 00 0B"),
            "{e}"
        );
    }

    #[test]
    fn wrong_opcode_is_protocol_error() {
        let mut s = session(MockTransport::new().reply(&E0A0));
        let e = s.identify().unwrap_err();
        assert_eq!(e.exit_code(), 3);
    }

    #[test]
    fn reply_longer_than_length_is_protocol_error() {
        let mut long = E0A0.to_vec();
        long.push(0xFF);
        let mut s = session(MockTransport::new().reply(&long));
        assert_eq!(s.basic_status().unwrap_err().exit_code(), 3);
    }

    #[test]
    fn stale_bytes_are_drained_before_the_first_command() {
        let m = MockTransport::new()
            .reply(&[0xA0, 0xE0, 0x0C, 0x00])
            .reply_split(&A1A1);
        let mut s = session(m);
        // the mock returns queued transfers regardless of timing, so drain exactly one round
        assert_eq!(s.drain_stale(Duration::from_millis(1), 1).unwrap(), 4);
        assert!(s.identify().is_ok());
        assert_eq!(s.drain_stale(Duration::from_millis(1), 5).unwrap(), 0);
    }

    #[test]
    fn dump_writes_tx_rx_files() {
        let dir = std::env::temp_dir().join(format!("canon-print-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session(MockTransport::new().reply_split(&A1A1))
            .with_dump(Some(Dumper::create(&dir).unwrap()));
        s.identify().unwrap();
        assert_eq!(
            std::fs::read(dir.join("tx-0001.bin")).unwrap(),
            vec![0xA1, 0xA1, 0x04, 0x00]
        );
        assert_eq!(
            std::fs::read(dir.join("rx-0001.bin")).unwrap(),
            A1A1.to_vec()
        );
        let log = std::fs::read_to_string(dir.join("protocol.log")).unwrap();
        assert!(log.contains("TX 0001 IDENT") && log.contains("RX 0001 IDENT"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
