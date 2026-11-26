//! How a Location of this technology is configured: `Configured`, reading
//! the settings its [`declaration`](crate::declaration) declares once
//! (ADR-0064, amendments 2026-09-26 and 2026-11-26).

use ethernet::Mac;
use transport::Configured;
use transport::error::{Result, protocol_error};
use xcore::settings::{Read, Settings};

use crate::{EtherCatTransport, declaration};

impl Configured for EtherCatTransport {
    const SETTINGS: &'static Settings = declaration::SETTINGS;
    const SCHEMES: &'static [&'static str] = declaration::SCHEMES;

    /// The station, index and subindex settings: the address names only the
    /// link.
    fn own_target(_address: &str) -> &str {
        transport::destination::OWN
    }

    fn timeout_kept(&mut self) -> Option<&mut dyn transport::configured::TimeoutKept> {
        Some(&mut self.timeout)
    }

    fn configured(address: &str, settings: &Read) -> Result<Self> {
        let link = ethernet::open_link(address)?;
        let source: Mac = settings.text("source").parse()?;
        let out_of_range = |name: &str| protocol_error(format!("{name} out of range"));
        let station =
            u16::try_from(settings.integer("station")).map_err(|_| out_of_range("station"))?;
        let index = u16::try_from(settings.integer("index")).map_err(|_| out_of_range("index"))?;
        let subindex =
            u8::try_from(settings.integer("subindex")).map_err(|_| out_of_range("subindex"))?;
        Ok(Self::new(link, source, station).about(index, subindex))
    }
}
