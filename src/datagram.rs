//! The `EtherCAT` datagram, IEC 61158-4-12: a command, an index the master
//! matches the answer by, an address, the data, and the working counter
//! every slave that acted on it increments. Several ride in one Ethernet
//! frame under the two-byte `EtherCAT` header.
//!
//! The address is two things: for a position or a fixed command it is a
//! sixteen-bit station — a position every slave increments as the frame
//! passes, or the address a slave was configured with — and a sixteen-bit
//! offset into that slave's memory; for a logical command it is thirty-two
//! bits into the memory the fieldbus memory management units map.

use transport::error::{Result, protocol_error};

/// The `EtherType` every `EtherCAT` frame carries.
pub const ETHERTYPE: u16 = 0x88a4;

/// What one standard frame carries after the `EtherCAT` header.
pub const FRAME_DATA_MAX: usize = ethernet::MTU - 2;

/// The command, index, address, length and interrupt bytes before the data,
/// and the working counter after it.
pub const DATAGRAM_OVERHEAD: usize = 12;

/// The data one datagram carries when it is alone in a standard frame.
pub const DATAGRAM_DATA_MAX: usize = FRAME_DATA_MAX - DATAGRAM_OVERHEAD;

/// The eleven bits a length field has.
const LENGTH_MASK: u16 = 0x07ff;

/// What a datagram asks of the slaves it reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Nop = 0,
    /// Auto increment physical read: the slave at position 0 answers.
    Aprd = 1,
    Apwr = 2,
    Aprw = 3,
    /// Configured address physical read: the slave with that station answers.
    Fprd = 4,
    Fpwr = 5,
    Fprw = 6,
    /// Broadcast: every slave answers, and the working counter counts them.
    Brd = 7,
    Bwr = 8,
    Brw = 9,
    /// Logical: the memory the FMMUs map.
    Lrd = 10,
    Lwr = 11,
    Lrw = 12,
    Armw = 13,
    Frmw = 14,
}

impl Command {
    /// The command a code names.
    ///
    /// # Errors
    /// A code outside the fifteen.
    pub fn from_code(code: u8) -> Result<Self> {
        Ok(match code {
            0 => Self::Nop,
            1 => Self::Aprd,
            2 => Self::Apwr,
            3 => Self::Aprw,
            4 => Self::Fprd,
            5 => Self::Fpwr,
            6 => Self::Fprw,
            7 => Self::Brd,
            8 => Self::Bwr,
            9 => Self::Brw,
            10 => Self::Lrd,
            11 => Self::Lwr,
            12 => Self::Lrw,
            13 => Self::Armw,
            14 => Self::Frmw,
            other => {
                return Err(protocol_error(format!(
                    "no EtherCAT command has code {other}"
                )));
            }
        })
    }

    /// True where the slave fills the data in.
    #[must_use]
    pub const fn reads(self) -> bool {
        matches!(
            self,
            Self::Aprd
                | Self::Aprw
                | Self::Fprd
                | Self::Fprw
                | Self::Brd
                | Self::Brw
                | Self::Lrd
                | Self::Lrw
                | Self::Armw
                | Self::Frmw
        )
    }

    /// True where the slave takes the data.
    #[must_use]
    pub const fn writes(self) -> bool {
        matches!(
            self,
            Self::Apwr
                | Self::Aprw
                | Self::Fpwr
                | Self::Fprw
                | Self::Bwr
                | Self::Brw
                | Self::Lwr
                | Self::Lrw
        )
    }
}

/// One datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub command: Command,
    /// The master's mark, echoed so an answer is matched to its question.
    pub index: u8,
    pub address: u32,
    pub interrupt: u16,
    pub data: Vec<u8>,
    /// How many slaves acted on it: zero on the way out.
    pub working_counter: u16,
}

impl Datagram {
    /// A datagram, refusing data over [`DATAGRAM_DATA_MAX`].
    ///
    /// # Errors
    /// Data that no frame carries.
    pub fn new(command: Command, address: u32, data: &[u8]) -> Result<Self> {
        if data.len() > DATAGRAM_DATA_MAX {
            return Err(protocol_error(format!(
                "{} bytes is over the {DATAGRAM_DATA_MAX} one datagram carries",
                data.len()
            )));
        }
        Ok(Self {
            command,
            index: 0,
            address,
            interrupt: 0,
            data: data.to_vec(),
            working_counter: 0,
        })
    }

    /// A position or fixed command at `station`, `offset` into its memory.
    ///
    /// # Errors
    /// As [`Datagram::new`].
    pub fn at(command: Command, station: u16, offset: u16, data: &[u8]) -> Result<Self> {
        Self::new(
            command,
            u32::from(station) | (u32::from(offset) << 16),
            data,
        )
    }

    /// The station half of a physical address.
    #[must_use]
    pub const fn station(&self) -> u16 {
        (self.address & 0xffff) as u16
    }

    /// The offset half of a physical address.
    #[must_use]
    pub const fn offset(&self) -> u16 {
        (self.address >> 16) as u16
    }

    /// The station half replaced, as a slave passing a position command
    /// increments it.
    pub const fn set_station(&mut self, station: u16) {
        self.address = (self.address & 0xffff_0000) | station as u32;
    }
}

/// `datagrams` as the payload of one frame, under the `EtherCAT` header.
///
/// # Errors
/// No datagrams, or more than one frame carries.
pub fn encode(datagrams: &[Datagram]) -> Result<Vec<u8>> {
    if datagrams.is_empty() {
        return Err(protocol_error("a frame with no datagram"));
    }
    let length: usize = datagrams
        .iter()
        .map(|datagram| DATAGRAM_OVERHEAD + datagram.data.len())
        .sum();
    if length > FRAME_DATA_MAX {
        return Err(protocol_error(format!(
            "{length} bytes of datagrams is over the {FRAME_DATA_MAX} one frame carries"
        )));
    }
    let mut out = Vec::with_capacity(2 + length);
    let header = u16::try_from(length).unwrap_or(0) | 0x1000;
    out.extend_from_slice(&header.to_le_bytes());
    let last = datagrams.len() - 1;
    for (n, datagram) in datagrams.iter().enumerate() {
        out.push(datagram.command as u8);
        out.push(datagram.index);
        out.extend_from_slice(&datagram.address.to_le_bytes());
        let mut len = u16::try_from(datagram.data.len()).unwrap_or(0) & LENGTH_MASK;
        if n < last {
            len |= 0x8000;
        }
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&datagram.interrupt.to_le_bytes());
        out.extend_from_slice(&datagram.data);
        out.extend_from_slice(&datagram.working_counter.to_le_bytes());
    }
    Ok(out)
}

/// The datagrams one frame's payload carries.
///
/// # Errors
/// A header that is not type 1, a length past the end, a command no slave
/// knows, or a last datagram that says more follow.
pub fn decode(payload: &[u8]) -> Result<Vec<Datagram>> {
    let header = payload
        .get(..2)
        .map(|h| u16::from_le_bytes([h[0], h[1]]))
        .ok_or_else(|| protocol_error("shorter than an EtherCAT header"))?;
    if header >> 12 != 1 {
        return Err(protocol_error("an EtherCAT header that is not type 1"));
    }
    let length = usize::from(header & LENGTH_MASK);
    let body = payload
        .get(2..2 + length)
        .ok_or_else(|| protocol_error("an EtherCAT length past the end of the frame"))?;
    let mut datagrams = Vec::new();
    let mut at = 0;
    loop {
        let fixed = body
            .get(at..at + 10)
            .ok_or_else(|| protocol_error("a datagram shorter than its header"))?;
        let len = u16::from_le_bytes([fixed[6], fixed[7]]);
        let data_len = usize::from(len & LENGTH_MASK);
        let more = len & 0x8000 != 0;
        let data = body
            .get(at + 10..at + 10 + data_len)
            .ok_or_else(|| protocol_error("a datagram length past the end"))?;
        let counter = body
            .get(at + 10 + data_len..at + 12 + data_len)
            .ok_or_else(|| protocol_error("a datagram without its working counter"))?;
        datagrams.push(Datagram {
            command: Command::from_code(fixed[0])?,
            index: fixed[1],
            address: u32::from_le_bytes([fixed[2], fixed[3], fixed[4], fixed[5]]),
            interrupt: u16::from_le_bytes([fixed[8], fixed[9]]),
            data: data.to_vec(),
            working_counter: u16::from_le_bytes([counter[0], counter[1]]),
        });
        at += DATAGRAM_OVERHEAD + data_len;
        if !more {
            break;
        }
    }
    if at != body.len() {
        return Err(protocol_error("bytes after the last datagram"));
    }
    Ok(datagrams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_datagrams_ride_one_frame_and_decode_back() {
        let mut first = Datagram::at(Command::Fpwr, 0x1001, 0x1000, &[1, 2, 3]).expect("fpwr");
        first.index = 7;
        let second = Datagram::at(Command::Fprd, 0x1001, 0x0130, &[0, 0]).expect("fprd");
        let payload = encode(&[first.clone(), second.clone()]).expect("encode");
        assert_eq!(&payload[..2], &[0x1d, 0x10], "29 bytes, type 1");
        assert_eq!(&payload[2..8], &[5, 7, 0x01, 0x10, 0x00, 0x10]);
        assert_eq!(&payload[8..10], &[0x03, 0x80], "three bytes, more follow");
        let back = decode(&payload).expect("decode");
        assert_eq!(back, vec![first, second]);
        assert_eq!(back[1].station(), 0x1001);
        assert_eq!(back[1].offset(), 0x0130);
    }

    #[test]
    fn a_position_is_incremented_as_a_slave_passes_it() {
        let mut datagram = Datagram::at(Command::Aprd, 0, 0x0010, &[0; 2]).expect("aprd");
        datagram.set_station(datagram.station().wrapping_add(1));
        assert_eq!(datagram.station(), 1);
        assert_eq!(datagram.offset(), 0x0010);
        assert!(Command::Aprd.reads());
        assert!(!Command::Aprd.writes());
        assert!(Command::Lrw.reads() && Command::Lrw.writes());
        assert!(!Command::Nop.reads() && !Command::Nop.writes());
        assert_eq!(Command::from_code(14).expect("frmw"), Command::Frmw);
    }

    #[test]
    fn what_is_not_an_ethercat_frame_is_refused() {
        assert!(decode(&[0x02]).is_err(), "short");
        assert!(
            decode(&[0x0c, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err(),
            "type 2"
        );
        assert!(decode(&[0x0c, 0x10, 0, 0]).is_err(), "length past the end");
        let mut bad = encode(&[Datagram::new(Command::Nop, 0, &[]).expect("nop")]).expect("ok");
        bad[2] = 15;
        assert!(decode(&bad).is_err(), "command 15");
        let mut more = encode(&[Datagram::new(Command::Nop, 0, &[]).expect("nop")]).expect("ok");
        more[9] = 0x80;
        assert!(decode(&more).is_err(), "the last says more follow");
        assert!(encode(&[]).is_err(), "no datagram");
        assert!(Datagram::new(Command::Lwr, 0, &[0; DATAGRAM_DATA_MAX + 1]).is_err());
        let one = Datagram::new(Command::Lwr, 0, &[0; DATAGRAM_DATA_MAX]).expect("brim");
        assert!(encode(&[one.clone(), one]).is_err(), "two at the brim");
        assert!(Command::from_code(15).is_err());
    }
}
