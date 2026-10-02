//! CAPT packet framing.
//!
//! `[opcode u16 LE][length u16 LE, includes the 4-byte header][payload]`
//! FACT: captdriver SPECS:26-38 + capt-command.c:122-137; libcapt PacketHeader.cpp:10-16.
//! Some firmware writes the length in BCD (57 -> `57 00`, captdriver SPECS:52-54);
//! a reply is accepted when its real size matches either reading, like captdriver.

use crate::error::{CanonError, Result};

pub const HEADER_LEN: usize = 4;
pub const MAX_PACKET_LEN: usize = 0xFFFF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub opcode: u16,
    pub payload: Vec<u8>,
}

fn protocol(msg: String) -> CanonError {
    CanonError::Protocol(msg)
}

/// Decimal reading of a BCD length (captdriver `BCD()`, word.h:39-49): `None` if any
/// nibble is not a decimal digit.
pub fn bcd_len(lo: u8, hi: u8) -> Option<usize> {
    let nibbles = [hi >> 4, hi & 0xF, lo >> 4, lo & 0xF];
    if nibbles.iter().any(|&n| n > 9) {
        return None;
    }
    Some(nibbles.iter().fold(0usize, |acc, &n| acc * 10 + n as usize))
}

impl Packet {
    pub fn new(opcode: u16, payload: Vec<u8>) -> Result<Packet> {
        if payload.len() + HEADER_LEN > MAX_PACKET_LEN {
            return Err(protocol(format!(
                "payload of {} bytes does not fit a CAPT packet (max {})",
                payload.len(),
                MAX_PACKET_LEN - HEADER_LEN
            )));
        }
        Ok(Packet { opcode, payload })
    }

    pub fn empty(opcode: u16) -> Packet {
        Packet {
            opcode,
            payload: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    pub fn is_empty(&self) -> bool {
        self.payload.is_empty()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(&self.opcode.to_le_bytes());
        out.extend_from_slice(&(self.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parse exactly one complete packet (a whole reply as received).
    pub fn parse(data: &[u8]) -> Result<Packet> {
        if data.len() < HEADER_LEN {
            return Err(protocol(format!(
                "reply too short: {} bytes, a CAPT header is 4 ({})",
                data.len(),
                hex(data)
            )));
        }
        let opcode = u16::from_le_bytes([data[0], data[1]]);
        let binary = u16::from_le_bytes([data[2], data[3]]) as usize;
        if binary < HEADER_LEN {
            return Err(protocol(format!(
                "invalid length field {binary} (< 4) in {}",
                hex(&data[..HEADER_LEN])
            )));
        }
        let ok = binary == data.len() || bcd_len(data[2], data[3]) == Some(data.len());
        if !ok {
            return Err(protocol(format!(
                "length field says {binary} bytes (BCD {:?}), received {}: {}",
                bcd_len(data[2], data[3]),
                data.len(),
                hex(&data[..data.len().min(16)])
            )));
        }
        Ok(Packet {
            opcode,
            payload: data[HEADER_LEN..].to_vec(),
        })
    }

    /// Parse a reply and check that it echoes the request opcode.
    pub fn parse_reply(data: &[u8], request: u16) -> Result<Packet> {
        let p = Packet::parse(data)?;
        if p.opcode != request {
            return Err(protocol(format!(
                "unexpected reply opcode 0x{:04X} to request 0x{request:04X}",
                p.opcode
            )));
        }
        Ok(p)
    }
}

/// Multi-command container (0xD0A9): payload = complete sub-packets, each with its
/// own header. FACT: capt-command.c:187-204. No reply is read for it.
pub fn multi(opcode: u16, parts: &[Packet]) -> Result<Packet> {
    let payload: Vec<u8> = parts.iter().flat_map(Packet::to_bytes).collect();
    Packet::new(opcode, payload)
}

/// Split a container payload back into its sub-packets (for decoding captures).
pub fn split_multi(payload: &[u8]) -> Result<Vec<Packet>> {
    let mut parts = Vec::new();
    let mut pos = 0;
    while pos < payload.len() {
        if payload.len() - pos < HEADER_LEN {
            return Err(protocol(format!(
                "truncated sub-packet header at offset {pos}"
            )));
        }
        let len = u16::from_le_bytes([payload[pos + 2], payload[pos + 3]]) as usize;
        if len < HEADER_LEN || pos + len > payload.len() {
            return Err(protocol(format!(
                "bad sub-packet length {len} at offset {pos}"
            )));
        }
        parts.push(Packet::parse(&payload[pos..pos + len])?);
        pos += len;
    }
    Ok(parts)
}

pub fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_ident_request() {
        // FACT: IDENT request on the wire is A1 A1 04 00
        assert_eq!(
            Packet::empty(0xA1A1).to_bytes(),
            vec![0xA1, 0xA1, 0x04, 0x00]
        );
    }

    #[test]
    fn serialize_fire_like_hw_log() {
        // HW (CUPS debug log, job 72): "CAPT: send  A7 E0 06 00 02 00"
        let p = Packet::new(0xE0A7, 2u16.to_le_bytes().to_vec()).unwrap();
        assert_eq!(p.to_bytes(), vec![0xA7, 0xE0, 0x06, 0x00, 0x02, 0x00]);
    }

    #[test]
    fn parse_status_reply_from_hw_log() {
        // HW (job 65): "CAPT: recv  A0 E0 0C 00 00 8A 00 00 0F 00 00 00"
        let raw = [
            0xA0, 0xE0, 0x0C, 0x00, 0x00, 0x8A, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00,
        ];
        let p = Packet::parse_reply(&raw, 0xE0A0).unwrap();
        assert_eq!(
            p.payload,
            vec![0x00, 0x8A, 0x00, 0x00, 0x0F, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn bcd_length_is_accepted() {
        assert_eq!(bcd_len(0x57, 0x00), Some(57));
        assert_eq!(bcd_len(0x5A, 0x00), None);
        let mut raw = vec![0xA1, 0xA1, 0x57, 0x00];
        raw.resize(57, 0);
        assert_eq!(Packet::parse(&raw).unwrap().payload.len(), 53);
    }

    #[test]
    fn malformed_replies_are_errors_not_panics() {
        assert!(Packet::parse(&[]).is_err()); // missing bytes
        assert!(Packet::parse(&[0xA0, 0xE0, 0x06]).is_err()); // short header
        assert!(Packet::parse(&[0xA0, 0xE0, 0x03, 0x00]).is_err()); // invalid length < 4
        assert!(Packet::parse(&[0xA0, 0xE0, 0x06, 0x00, 0x00]).is_err()); // missing byte
        assert!(Packet::parse(&[0xA0, 0xE0, 0x06, 0x00, 0, 0, 0]).is_err()); // extra byte
        let ok = [0xA0, 0xE0, 0x06, 0x00, 0x00, 0x8A];
        assert!(Packet::parse_reply(&ok, 0xA0A8).is_err()); // unexpected command
        assert!(Packet::new(0xC0A0, vec![0; MAX_PACKET_LEN]).is_err());
    }

    #[test]
    fn multi_container_roundtrip() {
        let parts = vec![
            Packet::new(0xD0A0, vec![1, 2, 3]).unwrap(),
            Packet::empty(0xD0A1),
            Packet::empty(0xD0A2),
        ];
        let m = multi(0xD0A9, &parts).unwrap();
        assert_eq!(m.len(), 4 + 7 + 4 + 4);
        assert_eq!(&m.to_bytes()[..4], &[0xA9, 0xD0, 19, 0]);
        assert_eq!(split_multi(&m.payload).unwrap(), parts);
        assert!(split_multi(&[0xA0, 0xD0, 0x09, 0x00]).is_err());
    }
}
