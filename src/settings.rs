//! The settings a Location of this technology takes, declared once and read through
//! (ADR-0064, amendment 2026-09-26).

use ethernet::Mac;
use transport::Configured;
use transport::error::{Result, protocol_error};
use xcore::settings::{Applies, Fixed, Kind, Presence, Read, Setting, Settings};

use crate::{EtherCatTransport, STREAM_OBJECT, TIMEOUT};

impl Configured for EtherCatTransport {
    /// The address is the Ethernet link the ring hangs off, by its name.
    /// This build carries the in-process link only, `loopback`, as the
    /// ethernet technology does; every other name is refused until a node
    /// implements a raw-socket link.
    const SETTINGS: &'static Settings = &Settings {
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
            Setting {
                name: "index",
                kind: Kind::Integer {
                    minimum: 0,
                    maximum: 0xffff,
                },
                presence: Presence::Default(Fixed::Integer(STREAM_OBJECT.0 as i64)),
                meaning: "The object's index in the slave's dictionary.",
                applies: Applies::Both,
            },
            Setting {
                name: "subindex",
                kind: Kind::Integer {
                    minimum: 0,
                    maximum: 255,
                },
                presence: Presence::Default(Fixed::Integer(STREAM_OBJECT.1 as i64)),
                meaning: "The object's subindex under its index.",
                applies: Applies::Both,
            },
            Setting {
                name: "timeout",
                kind: Kind::Duration,
                presence: Presence::Default(Fixed::Duration(TIMEOUT)),
                meaning: "How long a ring or a mailbox that stops answering is waited on.",
                applies: Applies::Both,
            },
        ],
    };

    /// The schemes a Location's address is entered with to choose this technology.
    const SCHEMES: &'static [&'static str] = &["ethercat"];

    fn configured(address: &str, settings: &Read) -> Result<Self> {
        let link = ethernet::open_link(address)?;
        let source: Mac = settings.text("source").parse()?;
        let out_of_range = |name: &str| protocol_error(format!("{name} out of range"));
        let station =
            u16::try_from(settings.integer("station")).map_err(|_| out_of_range("station"))?;
        let index = u16::try_from(settings.integer("index")).map_err(|_| out_of_range("index"))?;
        let subindex =
            u8::try_from(settings.integer("subindex")).map_err(|_| out_of_range("subindex"))?;
        Ok(Self::new(link, source, station)
            .about(index, subindex)
            .timing_out_after(settings.duration("timeout")))
    }
}
