//! A slave on an in-process segment: what a test or the loopback puts at
//! the far end so a master can be driven without a terminal in the room.
//!
//! Not a device. One slave has a station address, an AL status that says
//! operational, the two mailboxes, and the `CANopen` SDO server
//! (`canopen::sdo::server`) that `CoE` downloads and uploads reach, over
//! the [`COE`] window. As a frame passes, every datagram passes every slave: a slave
//! acts where the datagram addresses it, increments the working counter to
//! say so, and increments the position of an auto-increment command whether
//! it acted or not. [`Segment`] is the ring: the frame the master transmits
//! passes every slave in order and is what the master receives next.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use canopen::sdo::server::Server;
use canopen::sdo::{ABORT_COMMAND, Sdo};
use ethernet::{Frame, Link};
use transport::error::Result;

use crate::datagram::{self, Command, Datagram};
use crate::mailbox::{COE, MAILBOX_SIZE, Message, RECEIVE_MAILBOX, TRANSMIT_MAILBOX};

/// The register holding a slave's configured station address.
pub const STATION_ADDRESS: u16 = 0x0010;
/// The register holding a slave's application layer status.
pub const AL_STATUS: u16 = 0x0130;
/// The AL status of a slave that is operational.
pub const OPERATIONAL: u16 = 0x0008;

/// One slave.
pub struct Slave {
    station: u16,
    sdo: Server,
    transmit_mailbox: Option<Vec<u8>>,
}

impl Slave {
    /// A slave at `station`, operational, holding device type `0x0000_0000`
    /// at `0x1000:00` and nothing else.
    #[must_use]
    pub fn new(station: u16) -> Self {
        Self {
            station,
            sdo: Server::new(COE),
            transmit_mailbox: None,
        }
    }

    /// Hold `bytes` at `index:subindex`.
    #[must_use]
    pub fn with_object(mut self, index: u16, subindex: u8, bytes: impl Into<Vec<u8>>) -> Self {
        self.sdo.insert(index, subindex, bytes.into());
        self
    }

    /// The slave's station address.
    #[must_use]
    pub const fn station(&self) -> u16 {
        self.station
    }

    /// The bytes held at `index:subindex`, as they are now.
    #[must_use]
    pub fn object(&self, index: u16, subindex: u8) -> Option<&[u8]> {
        self.sdo.object(index, subindex)
    }

    /// One datagram passes: act where addressed, count, and increment a
    /// position.
    pub fn pass(&mut self, datagram: &mut Datagram) {
        let addressed = match datagram.command {
            Command::Aprd | Command::Apwr | Command::Aprw => {
                let here = datagram.station() == 0;
                datagram.set_station(datagram.station().wrapping_add(1));
                here
            }
            Command::Fprd | Command::Fpwr | Command::Fprw => datagram.station() == self.station,
            Command::Brd | Command::Bwr | Command::Brw => true,
            _ => false,
        };
        if !addressed {
            return;
        }
        let offset = datagram.offset();
        let acted = if datagram.command.writes() {
            self.write(offset, &datagram.data)
        } else {
            self.read(offset, &mut datagram.data)
        };
        if acted {
            datagram.working_counter = datagram.working_counter.wrapping_add(1);
        }
    }

    fn read(&mut self, offset: u16, data: &mut [u8]) -> bool {
        let bytes = match offset {
            STATION_ADDRESS => self.station.to_le_bytes().to_vec(),
            AL_STATUS => OPERATIONAL.to_le_bytes().to_vec(),
            TRANSMIT_MAILBOX => match self.transmit_mailbox.take() {
                Some(message) => message,
                None => return false,
            },
            _ => return false,
        };
        for (slot, byte) in data
            .iter_mut()
            .zip(bytes.iter().chain(std::iter::repeat(&0)))
        {
            *slot = *byte;
        }
        true
    }

    fn write(&mut self, offset: u16, data: &[u8]) -> bool {
        if offset != RECEIVE_MAILBOX || data.len() > MAILBOX_SIZE {
            return false;
        }
        let answer = match Message::decode(data) {
            Ok(message) => Message {
                counter: message.counter,
                sdo: self.sdo.serve(message.sdo),
            },
            Err(_) => Message {
                counter: 0,
                sdo: Sdo::Abort {
                    index: 0,
                    subindex: 0,
                    code: ABORT_COMMAND,
                },
            },
        };
        self.transmit_mailbox = answer.encode().ok();
        true
    }
}

/// A ring of slaves on an in-process link: the frame the master transmits
/// passes every slave in order and is what the master receives next.
pub struct Segment {
    slaves: Mutex<Vec<Slave>>,
    to_master: Mutex<VecDeque<Frame>>,
}

impl Segment {
    #[must_use]
    pub fn new(slaves: Vec<Slave>) -> Self {
        Self {
            slaves: Mutex::new(slaves),
            to_master: Mutex::new(VecDeque::new()),
        }
    }

    /// Look at the slave at `station`, where there is one.
    pub fn with_slave<R>(&self, station: u16, look: impl FnOnce(&Slave) -> R) -> Option<R> {
        self.slaves
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|slave| slave.station == station)
            .map(look)
    }
}

impl Link for Segment {
    fn name(&self) -> &'static str {
        "loopback"
    }

    fn receive(&self, _timeout: Duration) -> Result<Option<Frame>> {
        Ok(self
            .to_master
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front())
    }

    /// A frame that is not `EtherCAT`, or not well formed, is dropped the
    /// way a slave's port drops it: nothing comes back.
    fn transmit(&self, frame: &Frame) -> Result<()> {
        if frame.ethertype != datagram::ETHERTYPE {
            return Ok(());
        }
        let Ok(mut datagrams) = datagram::decode(&frame.payload) else {
            return Ok(());
        };
        {
            let mut slaves = self.slaves.lock().unwrap_or_else(PoisonError::into_inner);
            for datagram in &mut datagrams {
                for slave in slaves.iter_mut() {
                    slave.pass(datagram);
                }
            }
        }
        let payload = datagram::encode(&datagrams)?;
        let back = Frame::new(frame.source, frame.destination, frame.ethertype, &payload)?;
        self.to_master
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(back);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canopen::sdo::Opening;
    use ethernet::Mac;

    fn pass(segment: &Segment, datagrams: &[Datagram]) -> Vec<Datagram> {
        let payload = datagram::encode(datagrams).expect("encode");
        let frame = Frame::new(
            Mac::BROADCAST,
            Mac([2, 0, 0, 0, 0, 1]),
            datagram::ETHERTYPE,
            &payload,
        )
        .expect("frame");
        segment.transmit(&frame).expect("transmit");
        let back = segment
            .receive(Duration::ZERO)
            .expect("receive")
            .expect("a frame back");
        datagram::decode(&back.payload).expect("decode")
    }

    #[test]
    fn a_broadcast_counts_the_slaves_and_a_position_finds_the_second() {
        let segment = Segment::new(vec![Slave::new(0x1001), Slave::new(0x1002)]);
        let back = pass(
            &segment,
            &[
                Datagram::at(Command::Brd, 0, AL_STATUS, &[0; 2]).expect("brd"),
                Datagram::at(Command::Aprd, 0xffff, STATION_ADDRESS, &[0; 2]).expect("aprd"),
                Datagram::at(Command::Fprd, 0x1002, STATION_ADDRESS, &[0; 2]).expect("fprd"),
                Datagram::at(Command::Fprd, 0x1003, STATION_ADDRESS, &[0; 2]).expect("nobody"),
            ],
        );
        assert_eq!(
            back[0].working_counter, 2,
            "both slaves answered the broadcast"
        );
        assert_eq!(back[0].data, OPERATIONAL.to_le_bytes());
        assert_eq!(
            back[1].working_counter, 1,
            "position -1 is the second slave"
        );
        assert_eq!(back[1].data, 0x1002u16.to_le_bytes());
        assert_eq!(back[1].station(), 1, "incremented twice");
        assert_eq!(back[2].working_counter, 1);
        assert_eq!(back[3].working_counter, 0, "no such station");
        assert_eq!(segment.with_slave(0x1002, Slave::station), Some(0x1002));
        assert!(segment.with_slave(0x1003, Slave::station).is_none());
    }

    #[test]
    fn a_mailbox_write_is_answered_in_the_transmit_mailbox_and_a_bad_one_aborted() {
        let segment = Segment::new(vec![Slave::new(0x1001).with_object(0x2000, 0, Vec::new())]);
        let empty = pass(
            &segment,
            &[
                Datagram::at(Command::Fprd, 0x1001, TRANSMIT_MAILBOX, &[0; MAILBOX_SIZE])
                    .expect("fprd"),
            ],
        );
        assert_eq!(empty[0].working_counter, 0, "nothing to read yet");
        let ask = Message {
            counter: 1,
            sdo: Sdo::InitiateUpload {
                index: 0x1000,
                subindex: 0,
            },
        };
        let written = pass(
            &segment,
            &[Datagram::at(
                Command::Fpwr,
                0x1001,
                RECEIVE_MAILBOX,
                &ask.encode().expect("encode"),
            )
            .expect("fpwr")],
        );
        assert_eq!(written[0].working_counter, 1);
        let answer = pass(
            &segment,
            &[
                Datagram::at(Command::Fprd, 0x1001, TRANSMIT_MAILBOX, &[0; MAILBOX_SIZE])
                    .expect("fprd"),
            ],
        );
        assert_eq!(answer[0].working_counter, 1);
        assert_eq!(
            Message::decode(&answer[0].data).expect("message"),
            Message {
                counter: 1,
                sdo: Sdo::UploadOpened {
                    index: 0x1000,
                    subindex: 0,
                    opening: Opening::Sized {
                        size: 4,
                        data: vec![0; 4],
                    },
                },
            }
        );
        let garbage = pass(
            &segment,
            &[
                Datagram::at(Command::Fpwr, 0x1001, RECEIVE_MAILBOX, &[0xff; 8]).expect("fpwr"),
                Datagram::at(Command::Fprd, 0x1001, TRANSMIT_MAILBOX, &[0; MAILBOX_SIZE])
                    .expect("fprd"),
            ],
        );
        assert!(matches!(
            Message::decode(&garbage[1].data).expect("abort").sdo,
            Sdo::Abort {
                code: ABORT_COMMAND,
                ..
            }
        ));
        let frame =
            Frame::new(Mac::BROADCAST, Mac([2, 0, 0, 0, 0, 1]), 0x0800, &[0x45]).expect("ip");
        segment.transmit(&frame).expect("dropped");
        assert!(segment.receive(Duration::ZERO).expect("quiet").is_none());
    }
}
