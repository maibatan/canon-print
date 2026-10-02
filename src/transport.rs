//! Byte transport under the CAPT protocol.
//!
//! `UsbTransport` keeps the protocol independent of rusb so the session and state
//! machine can be tested with [`MockTransport`] (scripted printer replies, recorded
//! requests) without a printer.

use crate::error::{CanonError, Result};
use std::collections::VecDeque;
use std::time::Duration;

pub trait UsbTransport {
    /// Write all bytes (bulk OUT).
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<()>;
    /// One bulk IN transfer into `buf`; returns the bytes received.
    /// Returns `CanonError::Timeout` when nothing arrives in time.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize>;
    /// Max packet size of the bulk IN endpoint (a shorter transfer ends a reply).
    fn max_packet_size(&self) -> usize;
}

/// Scripted fake printer: every `read` returns the next queued transfer; an empty
/// queue behaves like a silent printer (timeout).
#[derive(Default)]
pub struct MockTransport {
    pub replies: VecDeque<Vec<u8>>,
    pub written: Vec<Vec<u8>>,
    pub max_packet: usize,
}

impl MockTransport {
    pub fn new() -> MockTransport {
        MockTransport {
            replies: VecDeque::new(),
            written: Vec::new(),
            max_packet: 512,
        }
    }

    /// Queue one bulk IN transfer.
    pub fn reply(mut self, transfer: &[u8]) -> MockTransport {
        self.replies.push_back(transfer.to_vec());
        self
    }

    /// Queue a complete reply the way the LBP2900 delivers it: the first 6 bytes as
    /// one transfer, then the rest (captdriver SPECS:49-50, studycapt capt.lua:53-63).
    pub fn reply_split(self, packet: &[u8]) -> MockTransport {
        if packet.len() <= 6 {
            return self.reply(packet);
        }
        self.reply(&packet[..6]).reply(&packet[6..])
    }
}

impl UsbTransport for MockTransport {
    fn write(&mut self, data: &[u8], _timeout: Duration) -> Result<()> {
        self.written.push(data.to_vec());
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        match self.replies.pop_front() {
            Some(t) => {
                if t.len() > buf.len() {
                    return Err(CanonError::Usb(format!(
                        "overflow: transfer of {} bytes into a {}-byte buffer",
                        t.len(),
                        buf.len()
                    )));
                }
                buf[..t.len()].copy_from_slice(&t);
                Ok(t.len())
            }
            None => Err(CanonError::Timeout {
                stage: "usb read".into(),
                elapsed: timeout,
                last_status: "no data".into(),
            }),
        }
    }

    fn max_packet_size(&self) -> usize {
        self.max_packet
    }
}

/// Protocol-level fake LBP2900 for state-machine tests. It parses every request and
/// answers like our unit did on the real hardware (docs/protocol.md): busy right after
/// IDENT, UNINIT until UPLOAD_2, page counters that advance after PRINT_DATA_END and
/// FIRE. Replies are delivered as a 6-byte transfer + the rest, like the printer.
pub struct FakePrinter {
    queue: VecDeque<Vec<u8>>,
    pub log: Vec<u16>,
    pub initialized: bool,
    pub no_paper: bool,
    /// When true, FIRE is accepted but the page never prints (like job 73).
    pub ignore_fire: bool,
    busy_polls: u32,
    decoding: u16,
    printing: u16,
    out: u16,
    completed: u16,
    received: u16,
    polls_since_fire: u32,
    fired: bool,
    pub data_bytes: usize,
    pub job_id: u16,
}

/// FACT (HW, canon-print status 2026-09-30): our unit's IDENT reply.
pub const HW_IDENT_REPLY: [u8; 56] = [
    0xA1, 0xA1, 0x38, 0x00, 0x00, 0x0B, 0x31, 0x2A, 0x01, 0x01, 0xF0, 0xFF, 0x40, 0x00, 0x04, 0x00,
    0x41, 0x00, 0x01, 0x00, 0xD0, 0x02, 0x00, 0x00, 0x6F, 0x08, 0x00, 0x00, 0xE4, 0x0D, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xFA, 0x02, 0x00, 0x00, 0xF6, 0x04, 0x00, 0x00, 0x28, 0x3C, 0x32, 0x32,
    0x58, 0x02, 0x58, 0x02, 0x15, 0x03, 0x02, 0x00,
];

impl Default for FakePrinter {
    fn default() -> Self {
        FakePrinter {
            queue: VecDeque::new(),
            log: Vec::new(),
            initialized: false,
            no_paper: false,
            ignore_fire: false,
            busy_polls: 0,
            decoding: 0,
            printing: 0,
            out: 0,
            completed: 0,
            received: 0,
            polls_since_fire: 0,
            fired: false,
            data_bytes: 0,
            job_id: 0x0042,
        }
    }
}

impl FakePrinter {
    fn status0(&self) -> u16 {
        let mut s: u16 = 0x8A00; // READY1 | bit 11 | JOBSTAT_CHNG, as seen on HW
        if !self.initialized {
            s |= 0x0031; // UNINIT1 | UNINIT2 | PROCESSING (HW 0x8A31)
        }
        if self.no_paper {
            s |= 0x0002;
        }
        s
    }

    fn push_reply(&mut self, op: u16, payload: &[u8]) {
        let len = (payload.len() + 4) as u16;
        let mut p = op.to_le_bytes().to_vec();
        p.extend_from_slice(&len.to_le_bytes());
        p.extend_from_slice(payload);
        if p.len() > 6 {
            self.queue.push_back(p[..6].to_vec());
            self.queue.push_back(p[6..].to_vec());
        } else {
            self.queue.push_back(p);
        }
    }

    /// Called on every A0A8: a fired page comes out after 2 polls, completes after 3.
    fn advance_after_fire(&mut self) {
        if !self.fired || self.no_paper {
            return;
        }
        self.polls_since_fire += 1;
        if self.polls_since_fire >= 2 {
            self.printing = self.decoding;
            self.out = self.decoding;
        }
        if self.polls_since_fire >= 3 {
            self.completed = self.decoding;
            self.fired = false;
        }
    }

    fn handle(&mut self, data: &[u8]) {
        if data.len() < 4 {
            return;
        }
        let op = u16::from_le_bytes([data[0], data[1]]);
        self.log.push(op);
        let echo = [0u8, 0];
        match op {
            0xA1A1 => {
                self.busy_polls = 1;
                let r = HW_IDENT_REPLY;
                self.queue.push_back(r[..6].to_vec());
                self.queue.push_back(r[6..].to_vec());
            }
            0xE0A0 => {
                if self.busy_polls > 0 {
                    self.busy_polls -= 1;
                    self.push_reply(op, &[0x88, 0x00]); // HW: busy right after IDENT
                } else {
                    let s = self.status0().to_le_bytes();
                    self.push_reply(op, &[s[0], s[1], 0, 0, 0x0F, 0, 0, 0]);
                }
            }
            0xA0A8 => {
                self.advance_after_fire();
                let mut p = vec![0u8; 52];
                p[..2].copy_from_slice(&self.status0().to_le_bytes());
                p[10] = 0x80;
                if self.no_paper {
                    p[9] = 0x40; // status1 NOPAPER2
                }
                for (off, v) in [
                    (14, self.decoding),
                    (16, self.printing),
                    (18, self.out),
                    (20, self.completed),
                    (34, self.received),
                ] {
                    p[off..off + 2].copy_from_slice(&v.to_le_bytes());
                }
                p[24] = 0x56;
                self.push_reply(op, &p);
            }
            0xA2A0 => {
                let id = self.job_id.to_le_bytes();
                self.push_reply(op, &[0, 0, id[0], id[1], 0, 0, 0, 0]);
            }
            0xE0A5 => {
                self.initialized = true;
                self.push_reply(op, &echo);
            }
            0xC0A0 => self.data_bytes += data.len() - 4,
            0xC0A4 => {
                self.decoding += 1;
                self.received = self.decoding;
            }
            0xD0A9 => {}
            0xE0A7 => {
                let page = data.get(4..6).map(|b| u16::from_le_bytes([b[0], b[1]]));
                // like job 73: a FIRE for the wrong page (or ignored) never prints
                if !self.ignore_fire && page == Some(self.decoding) {
                    self.fired = true;
                    self.polls_since_fire = 0;
                }
                self.push_reply(op, &echo);
            }
            _ => self.push_reply(op, &echo), // START_0, GPIO, JOB_SETUP, START_1..3, JOB_END
        }
    }
}

impl UsbTransport for FakePrinter {
    fn write(&mut self, data: &[u8], _timeout: Duration) -> Result<()> {
        self.handle(data);
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        match self.queue.pop_front() {
            Some(t) => {
                buf[..t.len()].copy_from_slice(&t);
                Ok(t.len())
            }
            None => Err(CanonError::Timeout {
                stage: "usb read".into(),
                elapsed: timeout,
                last_status: "no data".into(),
            }),
        }
    }

    fn max_packet_size(&self) -> usize {
        512
    }
}
