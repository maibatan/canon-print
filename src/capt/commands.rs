//! CAPT commands used by the LBP2900 job flow, as builders returning [`Packet`]s.
//!
//! Every opcode and payload layout comes from captdriver (CD, GPL-3.0) and is
//! documented in docs/protocol.md §3/§5. Canon names from libcapt (LC) / studycapt (SC)
//! are given where known; meanings marked INFERRED are not proven.

use super::packet::{multi, Packet};
use crate::compression::hiscoa::{self, Params};
use crate::error::Result;

/// Opcodes. FACT: CD capt-command.h:24-58.
pub mod op {
    /// Printer capabilities (LC PC_GET_PRINTER_INFO). Request: none. Reply: 56-byte info.
    pub const IDENT: u16 = 0xA1A1;
    /// Basic status (LC PCR_GET_BASIC_STATUS). Reply payload: STATUS0 u16 + 6 bytes on LBP2900 (HW).
    pub const CHKSTATUS: u16 = 0xE0A0;
    /// Extended status incl. page counters (CD CHKXSTATUS).
    pub const CHKXSTATUS: u16 = 0xA0A8;
    /// UNKNOWN meaning (CD START_0). Request: none.
    pub const START_0: u16 = 0xA3A2;
    /// Reserve the printer (LC PC_RESERVE_UNIT; CD JOB_BEGIN). Reply payload[2..4] = job id.
    pub const JOB_BEGIN: u16 = 0xA2A0;
    /// LED / button control (CD GPIO).
    pub const GPIO: u16 = 0xE1A2;
    /// Job progress information (CD JOB_SETUP; SC "tells the printer which job/stage").
    pub const JOB_SETUP: u16 = 0xE1A1;
    /// LC PCR_CLEAR_MISPRINT (CD START_1).
    pub const START_1: u16 = 0xE0A3;
    /// LC PCR_CLEAR_ERROR (CD START_2).
    pub const START_2: u16 = 0xE0A2;
    /// LC PCR_DISCARD_DATA (CD START_3).
    pub const START_3: u16 = 0xE0A4;
    /// LC PCR_GO_ONLINE (CD UPLOAD_2); clears UNINIT.
    pub const UPLOAD_2: u16 = 0xE0A5;
    /// Container for the page parameter packets. No reply.
    pub const SET_PARMS: u16 = 0xD0A9;
    /// Page parameters, 40 bytes (SC IC_BEGIN_PAGE).
    pub const SET_PARM_PAGE: u16 = 0xD0A0;
    /// Hi-SCoA parameters, 8 bytes (SC IC_BLACK_PLANE).
    pub const SET_PARM_HISCOA: u16 = 0xD0A4;
    /// SC IC_BEGIN_DATA. Empty.
    pub const SET_PARM_1: u16 = 0xD0A1;
    /// SC IC_END_PAGE. Empty.
    pub const SET_PARM_2: u16 = 0xD0A2;
    /// Hi-SCoA page data, at most 0xFF00 payload bytes per packet (SC IC_VIDEO_DATA). No reply.
    pub const PRINT_DATA: u16 = 0xC0A0;
    /// End of page data (SC IC_BLACK_END). No reply.
    pub const PRINT_DATA_END: u16 = 0xC0A4;
    /// Start printing page N (CD FIRE). Payload: page u16.
    pub const FIRE: u16 = 0xE0A7;
    /// Release the printer (LC PCR_RELEASE_UNIT; CD JOB_END). Payload: job id u16.
    pub const JOB_END: u16 = 0xE0A9;
}

/// Largest PRINT_DATA payload captdriver sends. FACT: generic-ops.c:44.
pub const PRINT_DATA_CHUNK: usize = 0xFF00;

pub fn ident() -> Packet {
    Packet::empty(op::IDENT)
}
pub fn chk_status() -> Packet {
    Packet::empty(op::CHKSTATUS)
}
pub fn chk_xstatus() -> Packet {
    Packet::empty(op::CHKXSTATUS)
}
pub fn start_0() -> Packet {
    Packet::empty(op::START_0)
}

/// FACT: CD magicbuf_0 (prn_lbp2900.c:52-54).
pub fn job_begin() -> Packet {
    Packet {
        opcode: op::JOB_BEGIN,
        payload: vec![0x00, 0x00, 0x1E, 0x00, 0x00, 0x00, 0x00, 0x00],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpio {
    /// What CD HEAD actually sends in the LBP2900 job prologue (16 bytes, `13 00 00` + zeros).
    Lbp3010Init,
    /// CD `lbp2900_gpio_init`: 12 zero bytes (used on cancel / after wait_user).
    Lbp2900Init,
    /// CD `lbp2900_gpio_blink` (asks the user to press the button).
    Lbp2900Blink,
}

pub fn gpio(kind: Gpio) -> Packet {
    let payload = match kind {
        Gpio::Lbp3010Init => {
            let mut v = vec![0u8; 16];
            v[0] = 0x13;
            v
        }
        Gpio::Lbp2900Init => vec![0u8; 12],
        Gpio::Lbp2900Blink => vec![
            0x00, 0x00, 0x01, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        ],
    };
    Packet {
        opcode: op::GPIO,
        payload,
    }
}

/// JOB_SETUP stage byte (`fg`, payload offset 16). FACT usage in CD, INFERRED meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStage {
    Start = 1,
    PageComplete = 2,
    Finished = 4,
    Fired = 6,
}

/// Local time written into JOB_SETUP (CD uses `localtime()`); passed in explicitly so
/// packet generation stays deterministic in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobTime {
    /// `tm_year`: years since 1900
    pub year_since_1900: u16,
    /// `tm_mon`: 0-based month
    pub month0: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// JOB_SETUP: 32-byte head + 40 zero bytes (empty host/user/document names).
/// FACT layout: CD send_job_start, prn_lbp2900.c:101-122.
pub fn job_setup(stage: JobStage, page: u16, job: u16, t: JobTime) -> Packet {
    let mut p = vec![0u8; 72];
    p[4..6].copy_from_slice(&page.to_le_bytes());
    p[16] = stage as u8;
    p[17] = 0x01;
    p[18..20].copy_from_slice(&job.to_le_bytes());
    p[20..22].copy_from_slice(&(-60i16).to_le_bytes()); // C4 FF, meaning UNKNOWN
    p[22..24].copy_from_slice(&(-120i16).to_le_bytes()); // 88 FF, meaning UNKNOWN
    p[24..26].copy_from_slice(&t.year_since_1900.to_le_bytes());
    p[26] = t.month0;
    p[27] = t.day;
    p[28] = t.hour;
    p[29] = t.minute;
    p[30] = t.second;
    p[31] = 0x01;
    Packet {
        opcode: op::JOB_SETUP,
        payload: p,
    }
}

pub fn start_1() -> Packet {
    Packet::empty(op::START_1)
}
pub fn start_2() -> Packet {
    Packet::empty(op::START_2)
}
pub fn start_3() -> Packet {
    Packet::empty(op::START_3)
}

/// FACT: CD magicbuf_2 = `EE DB EA AD` + 12 zero bytes (prn_lbp2900.c:56-59).
pub fn upload_2() -> Packet {
    let mut p = vec![0xEE, 0xDB, 0xEA, 0xAD];
    p.resize(16, 0);
    Packet {
        opcode: op::UPLOAD_2,
        payload: p,
    }
}

/// SET_PARM_PAGE fields. Byte layout FACT (CD prn_lbp2900.c:279-297); which values the
/// LBP2900 needs for each paper size is being verified (docs/protocol.md §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageParams {
    /// byte 4: paper size code (A4 = 0x02 in CD)
    pub size_code: u8,
    /// byte 8: toner density already shifted (`cupsInteger1 << 2`)
    pub toner_density: u8,
    /// byte 12: media type
    pub media_type: u8,
    /// byte 13: 0x11 for 600 dpi (0x81 for 400)
    pub media_adapt: u8,
    /// byte 18: "automatic image refinement" (CD 0x02)
    pub air: u8,
    /// byte 19: toner save 0/1
    pub toner_save: u8,
    /// bytes 22-23
    pub margin_height: u16,
    /// bytes 24-25
    pub margin_width: u16,
    /// bytes 26-27: raster bytes per line (= Hi-SCoA line size)
    pub line_size: u16,
    /// bytes 28-29: raster lines
    pub num_lines: u16,
    /// bytes 30-31: paper width in dots
    pub paper_width: u16,
    /// bytes 32-33: paper height in dots
    pub paper_height: u16,
    /// byte 36: fuser mode (plain paper 0x01)
    pub fuser_mode: u8,
}

impl PageParams {
    pub fn to_bytes(&self) -> [u8; 40] {
        let mut b = [0u8; 40];
        b[2] = 0x30; // model word 0x2A30, FACT (CD); A1A1 reports 0x2A31
        b[3] = 0x2A;
        b[4] = self.size_code;
        b[8] = self.toner_density;
        b[9] = 0x1C;
        b[10] = 0x1C;
        b[11] = 0x1C;
        b[12] = self.media_type;
        b[13] = self.media_adapt;
        b[14] = 0x04;
        b[15] = 0x00;
        b[16] = 0x01;
        b[17] = 0x01;
        b[18] = self.air;
        b[19] = self.toner_save;
        b[22..24].copy_from_slice(&self.margin_height.to_le_bytes());
        b[24..26].copy_from_slice(&self.margin_width.to_le_bytes());
        b[26..28].copy_from_slice(&self.line_size.to_le_bytes());
        b[28..30].copy_from_slice(&self.num_lines.to_le_bytes());
        b[30..32].copy_from_slice(&self.paper_width.to_le_bytes());
        b[32..34].copy_from_slice(&self.paper_height.to_le_bytes());
        b[36] = self.fuser_mode;
        b
    }
}

/// D0A9 { D0A0 page params, D0A4 Hi-SCoA params, D0A1, D0A2 } — 68 bytes for LBP2900.
/// FACT: CD prn_lbp2900.c:319-325.
pub fn set_parms(page: &PageParams, params: &Params) -> Result<Packet> {
    multi(
        op::SET_PARMS,
        &[
            Packet::new(op::SET_PARM_PAGE, page.to_bytes().to_vec())?,
            Packet::new(op::SET_PARM_HISCOA, hiscoa::format_params(params).to_vec())?,
            Packet::empty(op::SET_PARM_1),
            Packet::empty(op::SET_PARM_2),
        ],
    )
}

/// PRINT_DATA packets for one encoded band (split at 0xFF00 like CD generic-ops.c:40-54).
pub fn print_data(band: &[u8]) -> Vec<Packet> {
    band.chunks(PRINT_DATA_CHUNK)
        .map(|c| Packet {
            opcode: op::PRINT_DATA,
            payload: c.to_vec(),
        })
        .collect()
}

pub fn print_data_end() -> Packet {
    Packet::empty(op::PRINT_DATA_END)
}

pub fn fire(page: u16) -> Packet {
    Packet {
        opcode: op::FIRE,
        payload: page.to_le_bytes().to_vec(),
    }
}

pub fn job_end(job: u16) -> Packet {
    Packet {
        opcode: op::JOB_END,
        payload: job.to_le_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compression::hiscoa::DEFAULT_PARAMS;

    fn t() -> JobTime {
        JobTime {
            year_since_1900: 126,
            month0: 8,
            day: 29,
            hour: 21,
            minute: 45,
            second: 30,
        }
    }

    #[test]
    fn job_setup_matches_hw_log_prefix() {
        // HW (job 72): "CAPT: send  A1 E1 4C 00 00 00 00 00 02 00 00 00 00 00 00 00" = fg 2, page 2
        let b = job_setup(JobStage::PageComplete, 2, 7, t()).to_bytes();
        assert_eq!(b.len(), 76);
        assert_eq!(
            &b[..16],
            &[
                0xA1, 0xE1, 0x4C, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00
            ]
        );
        assert_eq!(b[4 + 16], 2); // fg
        assert_eq!(&b[4 + 20..4 + 24], &[0xC4, 0xFF, 0x88, 0xFF]);
        assert_eq!(&b[4 + 24..4 + 32], &[126, 0, 8, 29, 21, 45, 30, 1]);
    }

    #[test]
    fn fixed_payloads() {
        assert_eq!(
            job_begin().to_bytes(),
            vec![0xA0, 0xA2, 0x0C, 0x00, 0x00, 0x00, 0x1E, 0x00, 0, 0, 0, 0]
        );
        assert_eq!(upload_2().len(), 20);
        assert_eq!(&upload_2().payload[..4], &[0xEE, 0xDB, 0xEA, 0xAD]);
        assert_eq!(gpio(Gpio::Lbp3010Init).len(), 20);
        assert_eq!(gpio(Gpio::Lbp2900Init).len(), 16);
        assert_eq!(fire(2).to_bytes(), vec![0xA7, 0xE0, 0x06, 0x00, 0x02, 0x00]);
        assert_eq!(
            job_end(0x1234).to_bytes(),
            vec![0xA9, 0xE0, 0x06, 0x00, 0x34, 0x12]
        );
    }

    #[test]
    fn set_parms_layout() {
        let page = PageParams {
            size_code: 0x02,
            toner_density: 0,
            media_type: 0,
            media_adapt: 0x11,
            air: 0x02,
            toner_save: 0,
            margin_height: 0x78,
            margin_width: 0x60,
            line_size: 592,
            num_lines: 6776,
            paper_width: 4960,
            paper_height: 7014,
            fuser_mode: 0x01,
        };
        let b = set_parms(&page, &DEFAULT_PARAMS).unwrap().to_bytes();
        // FACT (CD): A9 D0 44 00 | A0 D0 2C 00 <40> | A4 D0 0C 00 <8> | A1 D0 04 00 | A2 D0 04 00
        assert_eq!(b.len(), 68);
        assert_eq!(&b[..8], &[0xA9, 0xD0, 0x44, 0x00, 0xA0, 0xD0, 0x2C, 0x00]);
        assert_eq!(&b[8..12], &[0x00, 0x00, 0x30, 0x2A]);
        assert_eq!(
            &b[48..60],
            &[0xA4, 0xD0, 0x0C, 0x00, 0x01, 0x04, 0x01, 0x01, 0x00, 0xF9, 0x00, 0x00]
        );
        assert_eq!(&b[60..], &[0xA1, 0xD0, 0x04, 0x00, 0xA2, 0xD0, 0x04, 0x00]);
        let p = &b[8..48];
        assert_eq!(u16::from_le_bytes([p[26], p[27]]), 592);
        assert_eq!(u16::from_le_bytes([p[32], p[33]]), 7014);
        assert_eq!(p[36], 0x01);
    }

    #[test]
    fn print_data_chunks_at_0xff00() {
        let band = vec![0u8; 0xFF00 + 10];
        let packets = print_data(&band);
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].payload.len(), 0xFF00);
        assert_eq!(packets[0].len(), 0xFF04);
        assert_eq!(packets[1].payload.len(), 10);
    }
}
