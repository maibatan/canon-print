//! Status and printer-info replies.
//!
//! Offsets FACT from captdriver (capt-status.c:53-79, SPECS:80-106) and studycapt
//! (A1A1 layout, CC0). Unlike captdriver, fields are only decoded when the reply is
//! long enough — captdriver reads page counters from uninitialized memory after a
//! short E0A0 reply (docs/protocol.md §4.3, pitfall 1).

use crate::error::{CanonError, Result};

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

/// STATUS0 bits. FACT: CD capt-status.h:34-63.
pub mod flag {
    pub const READY1: u16 = 1 << 15;
    pub const JOBSTAT_CHNG: u16 = 1 << 9;
    pub const XSTATUS_CHNG: u16 = 1 << 8;
    pub const BUSY: u16 = 1 << 7;
    pub const UNINIT1: u16 = 1 << 5;
    pub const UNINIT2: u16 = 1 << 4;
    pub const BUFFERFULL: u16 = 1 << 2;
    pub const NOPAPER1: u16 = 1 << 1;
    pub const PROCESSING: u16 = 1 << 0;
}

/// STATUS1 bits (extended status only). FACT: CD capt-status.h.
pub mod flag1 {
    pub const NOPAPER2: u16 = 1 << 14;
    pub const PROCESSING1: u16 = 1 << 7;
    pub const BUTTON: u16 = 1 << 5;
    pub const PRINTING: u16 = 1 << 2;
    pub const POWERUP: u16 = 1 << 0;
}

/// E0A0 reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicStatus {
    pub status0: u16,
    /// Remaining payload bytes (6 on our LBP2900, meaning UNKNOWN), kept for logs.
    pub rest: Vec<u8>,
}

impl BasicStatus {
    pub fn parse(payload: &[u8]) -> Result<BasicStatus> {
        if payload.len() < 2 {
            return Err(CanonError::Protocol(format!(
                "basic status reply has {} payload bytes, need 2",
                payload.len()
            )));
        }
        Ok(BasicStatus {
            status0: u16_at(payload, 0),
            rest: payload[2..].to_vec(),
        })
    }
    pub fn has(&self, f: u16) -> bool {
        self.status0 & f != 0
    }
    pub fn busy(&self) -> bool {
        self.has(flag::BUSY)
    }
    pub fn buffer_full(&self) -> bool {
        self.has(flag::BUFFERFULL)
    }
    pub fn uninitialized(&self) -> bool {
        self.has(flag::UNINIT1) || self.has(flag::UNINIT2)
    }
    pub fn xstatus_changed(&self) -> bool {
        self.has(flag::XSTATUS_CHNG)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PageCounters {
    pub decoding: u16,
    pub printing: u16,
    pub out: u16,
    pub completed: u16,
    pub received: u16,
}

/// A0A8 reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedStatus {
    /// status[0..=5]; status[6] only if the reply is long enough.
    pub status: [u16; 6],
    pub status6: Option<u16>,
    pub pages: PageCounters,
}

/// Payload bytes needed to read everything up to `page_received` (offset 34..35).
pub const XSTATUS_MIN_LEN: usize = 36;

impl ExtendedStatus {
    pub fn parse(payload: &[u8]) -> Result<ExtendedStatus> {
        if payload.len() < XSTATUS_MIN_LEN {
            return Err(CanonError::Protocol(format!(
                "extended status reply has {} payload bytes, need at least {XSTATUS_MIN_LEN} for the page counters",
                payload.len()
            )));
        }
        let p = payload;
        Ok(ExtendedStatus {
            status: [
                u16_at(p, 0),
                u16_at(p, 8),
                u16_at(p, 10),
                u16_at(p, 12),
                u16_at(p, 24),
                u16_at(p, 30),
            ],
            status6: (p.len() >= 40).then(|| u16_at(p, 38)),
            pages: PageCounters {
                decoding: u16_at(p, 14),
                printing: u16_at(p, 16),
                out: u16_at(p, 18),
                completed: u16_at(p, 20),
                received: u16_at(p, 34),
            },
        })
    }
    pub fn no_paper(&self) -> bool {
        self.status[0] & flag::NOPAPER1 != 0 || self.status[1] & flag1::NOPAPER2 != 0
    }
    pub fn printing(&self) -> bool {
        self.status[1] & (flag1::PRINTING | flag1::PROCESSING1) != 0
    }
}

/// A1A1 reply. Offsets are payload offsets = studycapt packet offsets − 4 (FACT for the
/// positions); the meanings are INFERRED by studycapt (diss_capt.py:89-167).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct PrinterInfo {
    /// 0x03 = CAPT 1.0/2.0, 0x0B = CAPT 2.1+
    pub info_format: u8,
    pub product_id: u16,
    pub firmware: (u8, u8),
    pub block_size: u16,
    pub buffers: u16,
    pub pages_per_hour: u16,
    /// max width / length, 0.1 mm
    pub max_width: u16,
    pub max_length: u16,
    pub min_width: u16,
    pub min_length: u16,
    /// top, bottom, left, right non-printable margins, 0.1 mm
    pub margins: [u8; 4],
    pub dpi: (u16, u16),
    pub version_b: u8,
}

pub const PRINTER_INFO_MIN_LEN: usize = 49;

impl PrinterInfo {
    pub fn parse(payload: &[u8]) -> Result<PrinterInfo> {
        if payload.len() < PRINTER_INFO_MIN_LEN {
            return Err(CanonError::Protocol(format!(
                "printer info reply has {} payload bytes, need {PRINTER_INFO_MIN_LEN}",
                payload.len()
            )));
        }
        let p = payload;
        Ok(PrinterInfo {
            info_format: p[1],
            product_id: u16_at(p, 2),
            firmware: (p[4], p[5]),
            block_size: u16_at(p, 6),
            buffers: u16_at(p, 8),
            pages_per_hour: u16_at(p, 16),
            max_width: u16_at(p, 20),
            max_length: u16_at(p, 24),
            min_width: u16_at(p, 32),
            min_length: u16_at(p, 36),
            margins: [p[40], p[41], p[42], p[43]],
            dpi: (u16_at(p, 44), u16_at(p, 46)),
            version_b: p[48],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capt::packet::Packet;

    /// LBP2900 A1A1 reply, studycapt docs/a1a1.py:36 (CC0, captdriver issue #7).
    const LBP2900_A1A1: [u8; 56] = [
        0xA1, 0xA1, 0x38, 0x00, 0x00, 0x0B, 0x31, 0x2A, 0x01, 0x01, 0xF0, 0xFF, 0x40, 0x00, 0x04,
        0x00, 0x41, 0x00, 0x01, 0x00, 0xD0, 0x02, 0x00, 0x00, 0x6F, 0x08, 0x00, 0x00, 0xE4, 0x0D,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFA, 0x02, 0x00, 0x00, 0xF6, 0x04, 0x00, 0x00, 0x28,
        0x3C, 0x32, 0x32, 0x58, 0x02, 0x58, 0x02, 0x15, 0x03, 0x02, 0x02,
    ];

    #[test]
    fn a1a1_golden_vector() {
        let p = Packet::parse_reply(&LBP2900_A1A1, 0xA1A1).unwrap();
        let i = PrinterInfo::parse(&p.payload).unwrap();
        assert_eq!(i.info_format, 0x0B);
        assert_eq!(i.product_id, 0x2A31);
        assert_eq!(i.firmware, (1, 1));
        assert_eq!(i.block_size, 0xFFF0);
        assert_eq!(i.buffers, 64);
        assert_eq!(i.pages_per_hour, 720); // 12 ppm, the LBP2900's rated speed
        assert_eq!((i.max_width, i.max_length), (2159, 3556)); // US Legal
        assert_eq!((i.min_width, i.min_length), (762, 1270));
        assert_eq!(i.margins, [40, 60, 50, 50]);
        assert_eq!(i.dpi, (600, 600));
        assert_eq!(i.version_b, 0x15);
    }

    #[test]
    fn basic_status_from_hw_log() {
        // HW job 65 payload: 00 8A 00 00 0F 00 00 00
        let s = BasicStatus::parse(&[0x00, 0x8A, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(s.status0, 0x8A00);
        assert!(!s.busy() && !s.buffer_full() && !s.uninitialized() && !s.xstatus_changed());
        assert!(s.has(flag::READY1) && s.has(flag::JOBSTAT_CHNG));
        let s = BasicStatus::parse(&[0x00, 0x8B]).unwrap();
        assert!(s.xstatus_changed());
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// FACT (HW, `canon-print status` on our unit, 2026-09-30, idle after power-on).
    const HW_A1A1: &str = "a1a13800000b312a0101f0ff4000040041000100d00200006f080000e40d000000000000fa020000f6040000283c32325802580215030200";
    const HW_E0A0_BUSY: &str = "a0e006008800";
    const HW_A0A8: &str = "a8a03800318a00000f0000000000800000000000000000000000000056000000000000000001000000000000000000000000000000000000";

    #[test]
    fn hw_ident_reply_of_our_unit() {
        let raw = unhex(HW_A1A1);
        let i = PrinterInfo::parse(&Packet::parse_reply(&raw, 0xA1A1).unwrap().payload).unwrap();
        assert_eq!(
            (i.info_format, i.product_id, i.firmware, i.dpi),
            (0x0B, 0x2A31, (1, 1), (600, 600))
        );
        // differs from the studycapt sample only in the last byte (02 there, 00 here)
        assert_eq!(&raw[..55], &LBP2900_A1A1[..55]);
        assert_eq!((raw[55], LBP2900_A1A1[55]), (0x00, 0x02));
    }

    #[test]
    fn hw_basic_status_busy_after_ident() {
        let p = Packet::parse_reply(&unhex(HW_E0A0_BUSY), 0xE0A0).unwrap();
        let b = BasicStatus::parse(&p.payload).unwrap();
        assert_eq!(b.status0, 0x0088);
        assert!(b.busy() && !b.uninitialized());
        assert!(b.rest.is_empty());
    }

    #[test]
    fn hw_extended_status_uninitialized_printer() {
        let p = Packet::parse_reply(&unhex(HW_A0A8), 0xA0A8).unwrap();
        assert_eq!(p.payload.len(), 52);
        let x = ExtendedStatus::parse(&p.payload).unwrap();
        assert_eq!(x.status[0], 0x8A31);
        assert!(x.status[0] & flag::UNINIT1 != 0 && x.status[0] & flag::UNINIT2 != 0);
        assert_eq!(x.status[2], 0x0080);
        assert_eq!(x.status[4], 0x0056);
        assert_eq!(
            x.pages.decoding + x.pages.out + x.pages.completed + x.pages.received,
            0
        );
        assert_eq!(p.payload[33], 0x01); // SPECS: number of E0A5 commands
        assert!(!x.no_paper() && !x.printing());
    }

    #[test]
    fn short_or_malformed_status_is_error() {
        assert!(BasicStatus::parse(&[0x00]).is_err());
        assert!(ExtendedStatus::parse(&[0u8; 35]).is_err());
        assert!(PrinterInfo::parse(&[0u8; 20]).is_err());
    }

    #[test]
    fn extended_status_counters() {
        let mut p = vec![0u8; 84];
        p[14..16].copy_from_slice(&2u16.to_le_bytes());
        p[16..18].copy_from_slice(&1u16.to_le_bytes());
        p[18..20].copy_from_slice(&1u16.to_le_bytes());
        p[20..22].copy_from_slice(&1u16.to_le_bytes());
        p[34..36].copy_from_slice(&2u16.to_le_bytes());
        p[9] = 0x40; // status1 bit 14 NOPAPER2
        let x = ExtendedStatus::parse(&p).unwrap();
        assert_eq!(
            x.pages,
            PageCounters {
                decoding: 2,
                printing: 1,
                out: 1,
                completed: 1,
                received: 2
            }
        );
        assert!(x.no_paper());
        assert!(x.status6.is_some());
        assert!(ExtendedStatus::parse(&p[..36]).unwrap().status6.is_none());
    }
}
