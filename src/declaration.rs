//! What a Location of this technology is configured with: its declaration,
//! which `Configured` reads, built with the crate's `declaration` feature
//! alone and the code behind the default (ADR-0064, amendments 2026-10-22
//! and 2026-11-26), for the library the surfaces load to carry (open
//! problem 48). It is the same on every operating system, those without
//! the kernel's raw socket included: a Location's settings are suggested
//! and checked wherever it is designed.

use std::time::Duration;

use xcore::settings::{Applies, Kind, Presence, Setting, Settings};

/// The object a Stream travels as unless a target says otherwise, and its
/// `index` and `subindex`: the mailbox reads the slave's dictionary as the
/// canopen technology does, so they are canopen's, declared once.
pub use canopen::declaration::{INDEX, STREAM_OBJECT, SUBINDEX};

/// How long a ring that stops answering is waited on when a Location says
/// nothing else.
pub const TIMEOUT: Duration = Duration::from_secs(1);

/// Every setting a Location takes beyond its address, the Ethernet link the
/// ring hangs off, by its name: `loopback`, the in-process link, or an
/// interface where the build carries the kernel's raw socket, as the
/// ethernet technology reads it.
pub const SETTINGS: &Settings = &Settings {
    technology: env!("CARGO_PKG_NAME"),
    settings: &[
        Setting {
            name: "source",
            kind: Kind::Address,
            presence: Presence::Required,
            meaning: "The MAC address the master's frames are sent from.",
            applies: Applies::Both,
        },
        Setting {
            name: "station",
            kind: Kind::Integer {
                minimum: 0,
                maximum: 0xffff,
            },
            presence: Presence::Required,
            meaning: "The configured station address of the slave a Location reads or \
                      writes.",
            applies: Applies::Both,
        },
        INDEX,
        SUBINDEX,
        transport::configured::timeout(
            Applies::Both,
            Some(TIMEOUT),
            "How long a ring or a mailbox that stops answering is waited on.",
        ),
    ],
};

/// The schemes a Location's address is entered with to choose this technology.
pub const SCHEMES: &[&str] = &["ethercat"];
