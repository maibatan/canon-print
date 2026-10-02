//! Real USB transport (libusb via rusb).
//!
//! Opens the printer, finds the bulk endpoints from the descriptors (never hard-coded),
//! and claims the interface. If a kernel driver (usblp) owns the interface, it is
//! detached ONLY when the caller explicitly allows it (`--detach-kernel-driver`), and
//! re-attached on drop — the same thing the CUPS usb backend does for every job.
//! No module is unloaded or blacklisted.
//!
//! A background thread keeps a bulk IN read pending at all times, exactly like the CUPS
//! usb backend's read thread (usb-libusb.c:1822-1839): one max-packet-sized (512-byte)
//! transfer at a time. Measured on our VMware VM: captdriver through CUPS got each reply
//! ~2 ms after the request (CUPS log, job 69); canon-print with 64 KiB reads got each
//! reply ~100 ms after the request, with or without a pending read (canon-print logs,
//! 2026-09-30). With 512-byte reads each reply arrives 2-3 ms after the request
//! (canon-print status log, same VM) — FACT: the transfer size caused the 100 ms.

use crate::error::{CanonError, Result};
use crate::transport::UsbTransport;
use crate::usb::{self, DeviceInfo};
use rusb::UsbContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

type Handle = rusb::DeviceHandle<rusb::Context>;

/// How long one pending IN transfer lives before it is re-submitted; also the
/// longest the reader needs to stop when the transport is dropped.
const READER_TRANSFER_TIMEOUT: Duration = Duration::from_secs(1);

pub struct RusbTransport {
    handle: Option<Arc<Handle>>,
    interface: u8,
    ep_in: u8,
    ep_out: u8,
    max_packet: usize,
    detached: bool,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    incoming: Receiver<Result<Vec<u8>>>,
    pub info: DeviceInfo,
}

fn start_reader(
    handle: Arc<Handle>,
    ep_in: u8,
    max_packet: usize,
    stop: Arc<AtomicBool>,
) -> (JoinHandle<()>, Receiver<Result<Vec<u8>>>) {
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        // one max-packet read at a time, like CUPS (readbuffer[512])
        let mut buf = vec![0u8; max_packet];
        while !stop.load(Ordering::Relaxed) {
            match handle.read_bulk(ep_in, &mut buf, READER_TRANSFER_TIMEOUT) {
                Ok(0) | Err(rusb::Error::Timeout) => continue,
                Ok(n) => {
                    if tx.send(Ok(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.into()));
                    break;
                }
            }
        }
    });
    (thread, rx)
}

fn parse_ep(addr: &str) -> Option<u8> {
    u8::from_str_radix(addr.trim_start_matches("0x"), 16).ok()
}

impl RusbTransport {
    /// Open the first Canon LBP candidate.
    pub fn open(detach_kernel_driver: bool) -> Result<RusbTransport> {
        let info = usb::enumerate()?
            .into_iter()
            .find(|d| d.is_candidate)
            .ok_or_else(|| CanonError::Usb("no Canon LBP printer found on USB".into()))?;

        // interface that has a bulk IN + OUT pair
        let (interface, ep_in, ep_out, max_packet) = info
            .configs
            .iter()
            .flat_map(|c| &c.interfaces)
            .find_map(|i| {
                let bulk = |dir: &str| {
                    i.endpoints
                        .iter()
                        .find(|e| e.transfer_type == "bulk" && e.direction == dir)
                };
                match (bulk("in"), bulk("out")) {
                    (Some(a), Some(b)) => Some((
                        i.number,
                        parse_ep(&a.address)?,
                        parse_ep(&b.address)?,
                        a.max_packet_size as usize,
                    )),
                    _ => None,
                }
            })
            .ok_or_else(|| CanonError::Usb("printer has no bulk IN/OUT interface".into()))?;

        let device = usb::context()?
            .devices()?
            .iter()
            .find(|d| d.bus_number() == info.bus && d.address() == info.address)
            .ok_or_else(|| CanonError::Usb("printer disappeared while opening".into()))?;
        let handle = device.open()?;

        let mut detached = false;
        if handle.kernel_driver_active(interface).unwrap_or(false) {
            let driver = info.kernel_drivers().join(", ");
            if !detach_kernel_driver {
                return Err(CanonError::Permission(format!(
                    "USB interface {interface} already claimed by another driver ({}). \
                     Re-run with --detach-kernel-driver (detached for this run only, re-attached \
                     afterwards), or unload it manually: sudo modprobe -r usblp",
                    if driver.is_empty() { "kernel" } else { &driver }
                )));
            }
            handle.detach_kernel_driver(interface)?;
            detached = true;
        }
        if let Err(e) = handle.claim_interface(interface) {
            if detached {
                let _ = handle.attach_kernel_driver(interface);
            }
            return Err(match e {
                rusb::Error::Busy => CanonError::Permission(format!(
                    "USB interface {interface} is busy (is CUPS printing right now?)"
                )),
                other => other.into(),
            });
        }
        let handle = Arc::new(handle);
        let stop = Arc::new(AtomicBool::new(false));
        let max_packet = max_packet.max(64);
        let (reader, incoming) =
            start_reader(Arc::clone(&handle), ep_in, max_packet, Arc::clone(&stop));
        Ok(RusbTransport {
            handle: Some(handle),
            interface,
            ep_in,
            ep_out,
            max_packet,
            detached,
            stop,
            reader: Some(reader),
            incoming,
            info,
        })
    }

    fn handle(&self) -> &Handle {
        self.handle.as_ref().expect("handle present until drop")
    }

    pub fn endpoints(&self) -> (u8, u8, u8) {
        (self.interface, self.ep_in, self.ep_out)
    }

    pub fn detached_kernel_driver(&self) -> bool {
        self.detached
    }
}

impl Drop for RusbTransport {
    fn drop(&mut self) {
        // stop the reader (its pending transfer ends within READER_TRANSFER_TIMEOUT),
        // then release the interface and give it back to the kernel driver
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        if let Some(arc) = self.handle.take() {
            if let Ok(h) = Arc::try_unwrap(arc) {
                let _ = h.release_interface(self.interface);
                if self.detached {
                    let _ = h.attach_kernel_driver(self.interface);
                }
            }
        }
    }
}

impl UsbTransport for RusbTransport {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<()> {
        let mut sent = 0;
        while sent < data.len() {
            match self
                .handle()
                .write_bulk(self.ep_out, &data[sent..], timeout)
            {
                Ok(n) => sent += n,
                Err(rusb::Error::Timeout) => {
                    return Err(CanonError::Timeout {
                        stage: "usb write".into(),
                        elapsed: timeout,
                        last_status: format!("{sent}/{} bytes sent", data.len()),
                    })
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        match self.incoming.recv_timeout(timeout) {
            Ok(Ok(data)) => {
                if data.len() > buf.len() {
                    return Err(CanonError::Usb(format!(
                        "received {} bytes, buffer holds {}",
                        data.len(),
                        buf.len()
                    )));
                }
                buf[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => Err(CanonError::Timeout {
                stage: "usb read".into(),
                elapsed: timeout,
                last_status: "no data".into(),
            }),
            Err(RecvTimeoutError::Disconnected) => {
                Err(CanonError::Usb("USB reader thread stopped".into()))
            }
        }
    }

    fn max_packet_size(&self) -> usize {
        self.max_packet
    }
}
