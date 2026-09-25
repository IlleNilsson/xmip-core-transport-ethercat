#![forbid(unsafe_code)]

//! Streams that are objects in an `EtherCAT` slave's dictionary. One
//! object — an index and a subindex on one station — is one Stream:
//! reading it is a `CoE` upload through the slave's mailbox, writing it a
//! download, and a Stream longer than one mailbox travels as an initiate and
//! segments under a toggle.
//!
//! `EtherCAT` is the machine's Ethernet with the switches taken out: one
//! frame from the master passes every slave in a ring, each reading and
//! writing its datagrams on the fly and incrementing a working counter to
//! say it did, and comes back. What is here is the datagram and the frame
//! that carries several ([`datagram`]), the mailbox and `CANopen` over it
//! ([`mailbox`]), a master that speaks them, and [`Slave`] and [`Segment`]
//! — a ring of slaves on an in-process link for tests and the loopback.
//! The carrier is `xmip-core-transport-ethernet`: this crate rides its
//! [`Link`] and [`Frame`] under `EtherType` `0x88a4` rather than knowing a
//! wire of its own.
//!
//! The origin URI names the link, the station and the object:
//! `ethercat://<link>/0x1001/0x2000/0`. A target is the same, or a bare
//! `0x<station>/0x<index>/<sub>`, or nothing for the configured object.

pub mod datagram;
pub mod mailbox;
pub mod slave;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use canopen::sdo::{Sdo, client};
pub use datagram::{Command, Datagram};
use ethernet::{Frame, Link, Mac};
pub use mailbox::Message;
pub use slave::{Segment, Slave};
use transport::arrived::next_arrival;
use transport::error::{Result, protocol_error};
use transport::held::Held;
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::{Arrived, Directions, Transport};

use crate::mailbox::{COE, MAILBOX_SIZE, RECEIVE_MAILBOX, TRANSMIT_MAILBOX};
use crate::slave::{AL_STATUS, OPERATIONAL};

/// The object a Stream travels as unless a target says otherwise: the
/// first manufacturer-specific index.
pub const STREAM_OBJECT: (u16, u8) = (0x2000, 0);

/// The station the loopback's one slave is configured at.
pub const LOOPBACK_STATION: u16 = 0x1001;

/// The master's side of one link, addressing one station's one object.
#[derive(Clone)]
pub struct EtherCatTransport {
    link: Arc<dyn Link>,
    source: Mac,
    station: u16,
    index: u16,
    subindex: u8,
    timeout: Duration,
    next_index: Arc<AtomicU8>,
}

impl EtherCatTransport {
    /// A master on `link` sending from `source`, speaking to `station`
    /// about [`STREAM_OBJECT`].
    #[must_use]
    pub fn new(link: Arc<dyn Link>, source: Mac, station: u16) -> Self {
        Self {
            link,
            source,
            station,
            index: STREAM_OBJECT.0,
            subindex: STREAM_OBJECT.1,
            timeout: Duration::from_secs(1),
            next_index: Arc::new(AtomicU8::new(0)),
        }
    }

    /// Speak about `index:subindex` instead.
    #[must_use]
    pub const fn about(mut self, index: u16, subindex: u8) -> Self {
        self.index = index;
        self.subindex = subindex;
        self
    }

    /// Give up on a ring that stops answering.
    #[must_use]
    pub const fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `ethercat://<link>/0x<station>/0x<index>/<sub>`.
    #[must_use]
    pub fn origin(&self, station: u16, index: u16, subindex: u8) -> String {
        format!(
            "ethercat://{}/{station:#06x}/{index:#06x}/{subindex}",
            self.link.name()
        )
    }

    /// One frame round the ring: the datagrams as they came back, each
    /// with its working counter.
    ///
    /// # Errors
    /// More than one frame carries, or a ring that does not answer.
    pub fn exchange(&self, mut datagrams: Vec<Datagram>) -> Result<Vec<Datagram>> {
        let index = self.next_index.fetch_add(1, Ordering::Relaxed);
        for datagram in &mut datagrams {
            datagram.index = index;
        }
        let payload = datagram::encode(&datagrams)?;
        self.link.transmit(&Frame::new(
            Mac::BROADCAST,
            self.source,
            datagram::ETHERTYPE,
            &payload,
        )?)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(frame) = self.link.receive(self.timeout)? {
                if frame.ethertype == datagram::ETHERTYPE
                    && let Ok(back) = datagram::decode(&frame.payload)
                    && back.first().is_some_and(|first| first.index == index)
                {
                    return Ok(back);
                }
                continue;
            }
            if Instant::now() >= deadline {
                return Err(protocol_error(
                    "the ring did not answer before the deadline",
                ));
            }
            std::thread::yield_now();
        }
    }

    /// How many slaves are on the ring: a broadcast read's working counter.
    ///
    /// # Errors
    /// As [`Self::exchange`].
    pub fn count(&self) -> Result<u16> {
        let back = self.exchange(vec![Datagram::at(Command::Brd, 0, AL_STATUS, &[0; 2])?])?;
        Ok(back.first().map_or(0, |first| first.working_counter))
    }

    /// One mailbox call to `station`: the message into its receive
    /// mailbox, with its AL status read in the same frame, then its answer
    /// out of the transmit mailbox.
    ///
    /// # Errors
    /// A station that is not there, not operational, or does not answer.
    pub fn call(&self, station: u16, sdo: &Sdo) -> Result<Sdo> {
        let counter = (self.next_index.load(Ordering::Relaxed) & 0x07).max(1);
        let sdo = sdo.clone();
        let message = Message { counter, sdo }.encode()?;
        let back = self.exchange(vec![
            Datagram::at(Command::Fpwr, station, RECEIVE_MAILBOX, &message)?,
            Datagram::at(Command::Fprd, station, AL_STATUS, &[0; 2])?,
        ])?;
        if back.iter().any(|datagram| datagram.working_counter != 1) {
            return Err(protocol_error(format!(
                "station {station:#06x} is not on the ring"
            )));
        }
        let status = u16::from_le_bytes([back[1].data[0], back[1].data[1]]);
        if status != OPERATIONAL {
            return Err(protocol_error(format!(
                "station {station:#06x} is in AL state {status:#06x}"
            )));
        }
        let deadline = Instant::now() + self.timeout;
        loop {
            let read = Datagram::at(Command::Fprd, station, TRANSMIT_MAILBOX, &[0; MAILBOX_SIZE])?;
            let back = self.exchange(vec![read])?;
            if back[0].working_counter == 1 {
                return Ok(Message::decode(&back[0].data)?.sdo);
            }
            if Instant::now() >= deadline {
                return Err(protocol_error(
                    "the mailbox answer did not come before the deadline",
                ));
            }
        }
    }

    /// Write `bytes` to `index:subindex` on `station`.
    ///
    /// # Errors
    /// A slave that aborts, answers out of turn, or stops answering.
    pub fn download(&self, station: u16, index: u16, subindex: u8, bytes: &[u8]) -> Result<()> {
        client::download(&COE, index, subindex, bytes, |sdo| self.call(station, sdo))
    }

    /// Read `index:subindex` from `station`.
    ///
    /// # Errors
    /// A slave that aborts, answers out of turn, or stops answering.
    pub fn upload(&self, station: u16, index: u16, subindex: u8) -> Result<Vec<u8>> {
        client::upload(&COE, index, subindex, |sdo| self.call(station, sdo))
    }

    /// The station and object a target names, or the configured ones.
    fn resolve(&self, target: &str) -> Result<(u16, u16, u8)> {
        let path = match transport::socket::target("ethercat", target) {
            Some((_, path)) => path,
            None => target,
        };
        if path.is_empty() {
            return Ok((self.station, self.index, self.subindex));
        }
        let bad = || protocol_error(format!("{target:?} is not 0x<station>/0x<index>/<sub>"));
        let hex = |part: Option<&str>| {
            part.and_then(|hex| hex.strip_prefix("0x"))
                .and_then(|hex| u16::from_str_radix(hex, 16).ok())
                .ok_or_else(bad)
        };
        let mut parts = path.split('/');
        let station = hex(parts.next())?;
        let index = hex(parts.next())?;
        let subindex = parts.next().and_then(|n| n.parse().ok()).ok_or_else(bad)?;
        Ok((station, index, subindex))
    }
}

impl Transport for EtherCatTransport {
    fn name(&self) -> &'static str {
        "ethercat"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// One upload of the object: its bytes as one Stream.
    fn receive(&self) -> Result<Vec<Arrived>> {
        let bytes = self.upload(self.station, self.index, self.subindex)?;
        Ok(vec![Arrived::new(
            self.origin(self.station, self.index, self.subindex),
            bytes,
        )])
    }

    /// One download of `bytes` to the object `target` names.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let (station, index, subindex) = self.resolve(target)?;
        self.download(station, index, subindex, bytes)
    }
}

impl EtherCatTransport {
    /// Both ends on one in-process ring: a master and one slave at
    /// [`LOOPBACK_STATION`], operational, the Stream object empty, the
    /// loopback timeout on the master.
    #[must_use]
    pub fn loopback() -> Self {
        let slave =
            Slave::new(LOOPBACK_STATION).with_object(STREAM_OBJECT.0, STREAM_OBJECT.1, Vec::new());
        Self::new(
            Arc::new(Segment::new(vec![slave])),
            Mac([0x02, 0, 0, 0, 0, 1]),
            LOOPBACK_STATION,
        )
        .timing_out_after(LOOPBACK_TIMEOUT)
    }
}

/// A Stream of any length travels through the mailbox: the SDO size is
/// thirty-two bits, and no ceiling below that is a fact of the protocol.
impl Loopback for EtherCatTransport {
    /// The slave holding what the master wrote, until it is uploaded back.
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let master = self.clone();
        Ok(Box::new(Held::new(
            self.origin(self.station, self.index, self.subindex),
            move || next_arrival(master.receive()?, "nothing came back from the slave"),
        )))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        self.send(address, payload)
    }

    /// In order on one thread: the ring answers as the master transmits, so
    /// the download goes first and the upload reads it back.
    fn exchanges_in_order(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::payload::edge_payloads;

    #[test]
    fn a_loopback_round_downloads_through_the_mailbox_and_uploads_back() {
        let loopback = EtherCatTransport::loopback();
        let arrived = loopback.round(b"one mailbox").expect("round");
        assert_eq!(arrived.bytes, b"one mailbox");
        assert_eq!(arrived.origin_uri, "ethercat://loopback/0x1001/0x2000/0");
        let long: Vec<u8> = (0..3000u32)
            .map(|n| u8::try_from(n % 256).unwrap_or(0))
            .collect();
        assert_eq!(loopback.round(&long).expect("segments").bytes, long);
        assert!(loopback.ceiling().is_none());
        assert!(loopback.refuses(&long).is_none());
        assert_eq!(loopback.name(), "ethercat");
        assert!(loopback.claims().is_none());
        assert_eq!(loopback.count().expect("brd"), 1);
    }

    #[test]
    fn the_loopback_returns_the_edges_whole() {
        let loopback = EtherCatTransport::loopback();
        for (name, bytes) in edge_payloads() {
            let arrived = loopback
                .round(&bytes)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(arrived.bytes, bytes, "{name}");
        }
    }

    #[test]
    fn a_target_names_the_station_and_object_and_what_is_not_there_is_refused() {
        let segment = Arc::new(Segment::new(vec![
            Slave::new(0x1001),
            Slave::new(0x1002).with_object(0x2001, 3, vec![0]),
        ]));
        let master = EtherCatTransport::new(
            Arc::clone(&segment) as Arc<dyn Link>,
            Mac([2, 0, 0, 0, 0, 1]),
            0x1001,
        )
        .timing_out_after(Duration::from_millis(100));
        assert_eq!(master.count().expect("brd"), 2);
        master
            .send("ethercat://loopback/0x1002/0x2001/3", &[1, 2, 3])
            .expect("another station's object");
        assert_eq!(
            segment.with_slave(0x1002, |slave| slave.object(0x2001, 3).map(<[u8]>::to_vec)),
            Some(Some(vec![1, 2, 3]))
        );
        master
            .send("0x1001/0x1000/0", &[7, 0, 0, 0])
            .expect("bare target");
        assert_eq!(
            master.clone().about(0x1000, 0).receive().expect("upload")[0].bytes,
            [7, 0, 0, 0]
        );
        let error = master.send("", b"x").expect_err("no such object on 0x1001");
        assert!(error.message.contains("0x06020000"), "{error}");
        let error = master
            .send("0x1009/0x2000/0", b"x")
            .expect_err("no such station");
        assert!(error.message.contains("not on the ring"), "{error}");
        assert!(master.send("1001/0x2000/0", b"x").is_err(), "not hex");
        let quiet = EtherCatTransport::new(Arc::new(ethernet::Loopback::new()), Mac([2; 6]), 1)
            .timing_out_after(Duration::from_millis(20));
        assert_eq!(
            quiet.count().expect("own frame back"),
            0,
            "nobody on the link"
        );
    }
}
