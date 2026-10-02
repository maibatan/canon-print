use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "canon-print",
    version,
    about = "Direct USB printing for Canon LBP2900 (CAPT), no CUPS"
)]
pub struct Cli {
    /// Machine-readable JSON on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Verbose protocol logging (stderr)
    #[arg(short, long, global = true)]
    pub verbose: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List Canon LBP printers on USB
    List {
        /// Show every USB device, not only Canon LBP candidates
        #[arg(long)]
        all: bool,
    },
    /// Dump descriptors, interfaces and endpoints of the attached printer
    Inspect,
    /// Environment + USB diagnosis (never changes the host)
    Diagnose,
    /// Talk to the printer without printing: identify + basic + extended status
    Status {
        /// Temporarily detach the kernel driver (usblp) from the printer interface for
        /// this run; it is re-attached afterwards. Never unloads or blacklists modules.
        #[arg(long)]
        detach_kernel_driver: bool,
        /// Save every TX/RX CAPT packet into this directory
        #[arg(long)]
        dump_protocol: Option<PathBuf>,
    },
    /// Print a PDF (rendered with Ghostscript) or PBM file (one or more pages)
    Print {
        file: PathBuf,
        /// Number of collated copies, all in one job
        #[arg(long, default_value_t = 1)]
        copies: u32,
        /// Parse + encode only, never open USB
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "a4")]
        paper: String,
        #[arg(long, default_value_t = 600)]
        dpi: u32,
        /// Save every TX/RX CAPT packet into this directory
        #[arg(long)]
        dump_protocol: Option<PathBuf>,
        /// Save the generated CAPT stream to this file
        #[arg(long)]
        output_capt: Option<PathBuf>,
        /// Temporarily detach the kernel driver (usblp) for this job; re-attached afterwards
        #[arg(long)]
        detach_kernel_driver: bool,
    },
}
