//! The mailbox, and `CANopen` over `EtherCAT` in it: what a Stream larger
//! than one datagram travels as.
//!
//! A slave's mailbox is two windows of its memory the master writes and
//! reads with ordinary datagrams — a receive mailbox the master writes into
//! and a transmit mailbox it reads the answer from. What goes through them
//! is a six-byte mailbox header naming the protocol and a counter, then that
//! protocol's message; for `CoE` a two-byte header naming the service and
//! then an SDO as `CiA 301` shapes it, with the data stretched to the mailbox
//! rather than cut at eight bytes. A download of an object longer than one
//! mailbox is an initiate carrying the first bytes and then segments under a
//! toggle, as ETG.1000.6 section 5.6 says.

use transport::error::{Result, protocol_error};

/// Where the master writes: the receive mailbox's offset in a slave.
pub const RECEIVE_MAILBOX: u16 = 0x1000;
/// Where the master reads: the transmit mailbox's offset in a slave.
pub const TRANSMIT_MAILBOX: u16 = 0x1100;
/// The size of either mailbox.
pub const MAILBOX_SIZE: usize = 256;

/// Mailbox header, `CoE` header, and the SDO's eight bytes before data.
const INITIATE_OVERHEAD: usize = 6 + 2 + 8;
/// Mailbox header, `CoE` header, and the SDO's one command byte.
const SEGMENT_OVERHEAD: usize = 6 + 2 + 1;

/// The data an initiate carries beside its size.
pub const INITIATE_DATA: usize = MAILBOX_SIZE - INITIATE_OVERHEAD;
/// The data a segment carries.
pub const SEGMENT_DATA: usize = MAILBOX_SIZE - SEGMENT_OVERHEAD;

/// The mailbox type that says `CoE`.
const TYPE_COE: u8 = 3;
/// The `CoE` service of an SDO request and of its response.
const SDO_REQUEST: u8 = 2;
const SDO_RESPONSE: u8 = 3;

/// The object does not exist.
pub const ABORT_NO_OBJECT: u32 = 0x0602_0000;
/// The toggle bit was not alternated.
pub const ABORT_TOGGLE: u32 = 0x0503_0000;
/// The command specifier is not valid.
pub const ABORT_COMMAND: u32 = 0x0504_0001;

/// One SDO exchange as the mailbox carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sdo {
    /// A download opens with the whole size and the first bytes.
    DownloadInitiate {
        index: u16,
        subindex: u8,
        size: u32,
        data: Vec<u8>,
    },
    DownloadAccepted {
        index: u16,
        subindex: u8,
    },
    DownloadSegment {
        toggle: bool,
        data: Vec<u8>,
        last: bool,
    },
    SegmentAccepted {
        toggle: bool,
    },
    UploadInitiate {
        index: u16,
        subindex: u8,
    },
    /// An upload opens with the whole size and the first bytes.
    UploadOpened {
        index: u16,
        subindex: u8,
        size: u32,
        data: Vec<u8>,
    },
    UploadSegment {
        toggle: bool,
    },
    UploadData {
        toggle: bool,
        data: Vec<u8>,
        last: bool,
    },
    Abort {
        index: u16,
        subindex: u8,
        code: u32,
    },
}

impl Sdo {
    const fn is_request(&self) -> bool {
        matches!(
            self,
            Self::DownloadInitiate { .. }
                | Self::DownloadSegment { .. }
                | Self::UploadInitiate { .. }
                | Self::UploadSegment { .. }
        )
    }

    fn body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAILBOX_SIZE);
        match self {
            Self::DownloadInitiate {
                index,
                subindex,
                size,
                data,
            } => initiate(&mut out, 0x21, *index, *subindex, *size, data),
            Self::DownloadAccepted { index, subindex } => {
                initiate(&mut out, 0x60, *index, *subindex, 0, &[]);
            }
            Self::DownloadSegment { toggle, data, last }
            | Self::UploadData { toggle, data, last } => {
                out.push(toggle_bit(*toggle) | u8::from(*last));
                out.extend_from_slice(data);
            }
            Self::SegmentAccepted { toggle } => out.push(0x20 | toggle_bit(*toggle)),
            Self::UploadInitiate { index, subindex } => {
                initiate(&mut out, 0x40, *index, *subindex, 0, &[]);
            }
            Self::UploadOpened {
                index,
                subindex,
                size,
                data,
            } => initiate(&mut out, 0x41, *index, *subindex, *size, data),
            Self::UploadSegment { toggle } => out.push(0x60 | toggle_bit(*toggle)),
            Self::Abort {
                index,
                subindex,
                code,
            } => initiate(&mut out, 0x80, *index, *subindex, *code, &[]),
        }
        out
    }

    fn from_body(body: &[u8], request: bool) -> Result<Self> {
        let cs = *body
            .first()
            .ok_or_else(|| protocol_error("an SDO with no command specifier"))?;
        let toggle = cs & 0x10 != 0;
        let last = cs & 0x01 != 0;
        // Index, subindex, the size or abort code, and what follows: the
        // shape every initiate and abort has.
        let long = || -> Result<(u16, u8, u32, Vec<u8>)> {
            let head = body
                .get(1..8)
                .ok_or_else(|| protocol_error("an SDO initiate shorter than eight bytes"))?;
            Ok((
                u16::from_le_bytes([head[0], head[1]]),
                head[2],
                u32::from_le_bytes([head[3], head[4], head[5], head[6]]),
                body[8..].to_vec(),
            ))
        };
        Ok(match (cs >> 5, request) {
            (0, true) => Self::DownloadSegment {
                toggle,
                data: body[1..].to_vec(),
                last,
            },
            (0, false) => Self::UploadData {
                toggle,
                data: body[1..].to_vec(),
                last,
            },
            (1, true) => {
                let (index, subindex, size, data) = long()?;
                Self::DownloadInitiate {
                    index,
                    subindex,
                    size,
                    data,
                }
            }
            (1, false) => Self::SegmentAccepted { toggle },
            (2, true) => {
                let (index, subindex, _, _) = long()?;
                Self::UploadInitiate { index, subindex }
            }
            (2, false) => {
                let (index, subindex, size, data) = long()?;
                Self::UploadOpened {
                    index,
                    subindex,
                    size,
                    data,
                }
            }
            (3, true) => Self::UploadSegment { toggle },
            (3, false) => {
                let (index, subindex, _, _) = long()?;
                Self::DownloadAccepted { index, subindex }
            }
            (4, _) => {
                let (index, subindex, code, _) = long()?;
                Self::Abort {
                    index,
                    subindex,
                    code,
                }
            }
            (other, _) => {
                return Err(protocol_error(format!(
                    "an SDO command specifier of {other}"
                )));
            }
        })
    }
}

fn initiate(out: &mut Vec<u8>, cs: u8, index: u16, subindex: u8, word: u32, data: &[u8]) {
    out.push(cs);
    out.extend_from_slice(&index.to_le_bytes());
    out.push(subindex);
    out.extend_from_slice(&word.to_le_bytes());
    out.extend_from_slice(data);
}

const fn toggle_bit(toggle: bool) -> u8 {
    if toggle { 0x10 } else { 0x00 }
}

/// One mailbox message: a `CoE` SDO under a counter the slave uses to tell
/// a repeat from a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub counter: u8,
    pub sdo: Sdo,
}

impl Message {
    /// The message as the mailbox holds it: header, `CoE` header, SDO.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let body = self.sdo.body();
        let service = if self.sdo.is_request() {
            SDO_REQUEST
        } else {
            SDO_RESPONSE
        };
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&u16::try_from(2 + body.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.push(0);
        out.push(TYPE_COE | ((self.counter & 0x07) << 4));
        out.extend_from_slice(&(u16::from(service) << 12).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// The message a mailbox holds.
    ///
    /// # Errors
    /// Shorter than its headers, a length past the end, a protocol that is
    /// not `CoE`, or a service that is not an SDO.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 8 {
            return Err(protocol_error("a mailbox shorter than its headers"));
        }
        let length = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
        let body = bytes
            .get(8..6 + length)
            .ok_or_else(|| protocol_error("a mailbox length past the end"))?;
        if bytes[5] & 0x0f != TYPE_COE {
            return Err(protocol_error("a mailbox protocol that is not CoE"));
        }
        let counter = bytes[5] >> 4;
        let service = u8::try_from(u16::from_le_bytes([bytes[6], bytes[7]]) >> 12).unwrap_or(0);
        let request = match service {
            SDO_REQUEST => true,
            SDO_RESPONSE => false,
            other => return Err(protocol_error(format!("CoE service {other} is not an SDO"))),
        };
        Ok(Self {
            counter,
            sdo: Sdo::from_body(body, request)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(sdo: Sdo) {
        let message = Message { counter: 5, sdo };
        let bytes = message.encode();
        assert!(bytes.len() <= MAILBOX_SIZE, "fits the mailbox");
        assert_eq!(Message::decode(&bytes).expect("decode"), message);
    }

    #[test]
    fn a_download_opens_with_its_size_and_first_bytes_then_segments() {
        let open = Sdo::DownloadInitiate {
            index: 0x2000,
            subindex: 0,
            size: 1000,
            data: vec![7; INITIATE_DATA],
        };
        let bytes = Message {
            counter: 1,
            sdo: open.clone(),
        }
        .encode();
        assert_eq!(bytes.len(), MAILBOX_SIZE);
        assert_eq!(
            &bytes[..6],
            &[0xfa, 0x00, 0, 0, 0, 0x13],
            "250 bytes, CoE, counter 1"
        );
        assert_eq!(&bytes[6..8], &[0x00, 0x20], "SDO request");
        assert_eq!(&bytes[8..16], &[0x21, 0x00, 0x20, 0x00, 0xe8, 0x03, 0, 0]);
        round(open);
        round(Sdo::DownloadAccepted {
            index: 0x2000,
            subindex: 0,
        });
        round(Sdo::DownloadSegment {
            toggle: true,
            data: vec![9; SEGMENT_DATA],
            last: true,
        });
        round(Sdo::SegmentAccepted { toggle: true });
    }

    #[test]
    fn an_upload_is_the_mirror_and_an_abort_carries_its_reason() {
        round(Sdo::UploadInitiate {
            index: 0x1018,
            subindex: 1,
        });
        round(Sdo::UploadOpened {
            index: 0x1018,
            subindex: 1,
            size: 4,
            data: vec![1, 2, 3, 4],
        });
        round(Sdo::UploadSegment { toggle: false });
        round(Sdo::UploadData {
            toggle: false,
            data: Vec::new(),
            last: true,
        });
        let abort = Sdo::Abort {
            index: 0x9999,
            subindex: 0,
            code: ABORT_NO_OBJECT,
        };
        let bytes = Message {
            counter: 2,
            sdo: abort.clone(),
        }
        .encode();
        assert_eq!(&bytes[6..8], &[0x00, 0x30], "SDO response");
        round(abort);
    }

    #[test]
    fn what_is_not_a_coe_mailbox_is_refused() {
        assert!(Message::decode(&[0; 7]).is_err(), "short");
        assert!(
            Message::decode(&[0x09, 0, 0, 0, 0, 0x03, 0, 0x20]).is_err(),
            "past the end"
        );
        assert!(
            Message::decode(&[0x02, 0, 0, 0, 0, 0x02, 0, 0x20]).is_err(),
            "EoE"
        );
        assert!(
            Message::decode(&[0x02, 0, 0, 0, 0, 0x03, 0, 0x10]).is_err(),
            "emergency"
        );
        assert!(
            Message::decode(&[0x02, 0, 0, 0, 0, 0x03, 0, 0x20]).is_err(),
            "no specifier"
        );
        assert!(
            Message::decode(&[0x03, 0, 0, 0, 0, 0x03, 0, 0x20, 0xa0]).is_err(),
            "specifier 5"
        );
        assert!(
            Message::decode(&[0x03, 0, 0, 0, 0, 0x03, 0, 0x20, 0x21]).is_err(),
            "short initiate"
        );
    }
}
