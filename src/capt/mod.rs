//! CAPT protocol (LBP2900 flavour): framing, commands, status replies.
//! Printer session / state machine: next phases. Facts and sources: docs/protocol.md.

pub mod commands;
pub mod job;
pub mod lbp2900;
pub mod packet;
pub mod session;
pub mod status;
