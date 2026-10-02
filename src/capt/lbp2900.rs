//! LBP2900-specific values.
//!
//! Page geometry: what captdriver (commit 62719249) sends in SET_PARM_PAGE on our unit,
//! recovered by applying captdriver's paper.c/prn_lbp2900.c rules to the CUPS raster
//! header produced with the printer's PPD on the VM (Ubuntu 22.04, cups-filters,
//! `scripts/cups-raster-params.py`, 2026-09-30). The first 8 bytes of the A4 packet
//! also match the CUPS debug log of real job 78. These values printed successfully on
//! this printer (jobs 60-72); they are FACT for "works", not proof that every field is
//! required. Notable captdriver quirks kept on purpose:
//! - line_size = PageSize[0] in points used as bytes (A4 595 = 4760 dots)
//! - size code is 0x02 for A5 too (captdriver takes it from MediaType, not the size)

use super::commands::PageParams;
use crate::error::{CanonError, Result};

pub const USB_VENDOR_ID: u16 = 0x04a9;
pub const USB_PRODUCT_ID: u16 = 0x2676;
/// FACT (HW): product string reported by our unit.
pub const USB_PRODUCT_STRING: &str = "Canon CAPT USB Device";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    A4,
    A5,
}

impl Paper {
    pub fn parse(name: &str) -> Result<Paper> {
        match name.to_ascii_lowercase().as_str() {
            "a4" => Ok(Paper::A4),
            "a5" => Ok(Paper::A5),
            other => Err(CanonError::Input(format!(
                "paper '{other}' not supported yet (supported: a4, a5)"
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A5 => "A5",
        }
    }

    /// SET_PARM_PAGE values, 600 dpi, plain paper, no toner save.
    pub fn page_params(self) -> PageParams {
        let (line_size, num_lines, paper_width) = match self {
            Paper::A4 => (595, 6780, 4722),
            Paper::A5 => (420, 4722, 3264),
        };
        PageParams {
            size_code: 0x02,
            toner_density: 0,
            media_type: 0,
            media_adapt: 0x11,
            air: 0x02,
            toner_save: 0,
            margin_height: 14,
            margin_width: 14,
            line_size,
            num_lines,
            paper_width,
            paper_height: num_lines,
            fuser_mode: 0x01,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn a4_matches_captdriver_on_our_vm() {
        // scripts/cups-raster-params.py on the VM, A4, PPD CanonLBP-2900-3000.ppd
        let expected = hex(
            "00 00 30 2A 02 00 00 00 00 1C 1C 1C 00 11 04 00 01 01 02 00 00 00 0E 00 0E 00 53 02 7C 1A 72 12 7C 1A 00 00 01 00 00 00",
        );
        assert_eq!(Paper::A4.page_params().to_bytes().to_vec(), expected);
    }

    #[test]
    fn a4_prefix_matches_cups_log_of_job_78() {
        // HW: "CAPT: send  A9 D0 44 00 A0 D0 2C 00 00 00 30 2A 02 00 00 00"
        let pkt = super::super::commands::set_parms(
            &Paper::A4.page_params(),
            &crate::compression::hiscoa::DEFAULT_PARAMS,
        )
        .unwrap()
        .to_bytes();
        assert_eq!(
            &pkt[..16],
            &hex("A9 D0 44 00 A0 D0 2C 00 00 00 30 2A 02 00 00 00")[..]
        );
    }

    #[test]
    fn a5_matches_captdriver_on_our_vm() {
        let expected = hex(
            "00 00 30 2A 02 00 00 00 00 1C 1C 1C 00 11 04 00 01 01 02 00 00 00 0E 00 0E 00 A4 01 72 12 C0 0C 72 12 00 00 01 00 00 00",
        );
        assert_eq!(Paper::A5.page_params().to_bytes().to_vec(), expected);
    }

    #[test]
    fn a4_has_97_bands() {
        assert_eq!(Paper::A4.page_params().num_lines.div_ceil(70), 97);
    }

    #[test]
    fn paper_names() {
        assert_eq!(Paper::parse("A4").unwrap(), Paper::A4);
        assert_eq!(Paper::parse("a5").unwrap(), Paper::A5);
        assert!(Paper::parse("letter").is_err());
    }
}
