//! The mailbox, and `CANopen` over `EtherCAT` in it: what a Stream larger
//! than one datagram travels as.
//!
//! A slave's mailbox is two windows of its memory the master writes and
//! reads with ordinary datagrams — a receive mailbox the master writes into
//! and a transmit mailbox it reads the answer from. What goes through them
//! is a six-byte mailbox header naming the protocol and a counter, then that
//! protocol's message; for `CoE` a two-byte header naming the service and
//! then an SDO as `CiA 301` shapes it, with the data stretched to the mailbox
//! rather than cut at eight bytes. The SDO is the canopen technology's
//! ([`canopen::sdo`]): what is `EtherCAT`'s here is the mailbox, its headers
//! and the [`COE`] window. A download of an object longer than one mailbox
//! is an initiate carrying the first bytes and then segments under a toggle,
//! as ETG.1000.6 section 5.6 says.

use canopen::sdo::{Sdo, Window};
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

/// What one `CoE` SDO carries of an object: an initiate and a segment
/// stretched to the mailbox, and nothing expedited — the initiate carries
/// more.
pub const COE: Window = Window {
    expedited: 0,
    initiate: INITIATE_DATA,
    segment: SEGMENT_DATA,
};

/// The mailbox type that says `CoE`.
const TYPE_COE: u8 = 3;
/// The `CoE` service of an SDO request and of its response.
const SDO_REQUEST: u8 = 2;
const SDO_RESPONSE: u8 = 3;

/// One mailbox message: a `CoE` SDO under a counter the slave uses to tell
/// a repeat from a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub counter: u8,
    pub sdo: Sdo,
}

impl Message {
    /// The message as the mailbox holds it: header, `CoE` header, SDO.
    ///
    /// # Errors
    /// What [`Sdo::encode`] refuses.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let body = self.sdo.encode()?;
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
        Ok(out)
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
            sdo: Sdo::decode(body, request)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canopen::sdo::{ABORT_NO_OBJECT, Opening};

    fn round(sdo: Sdo) {
        let message = Message { counter: 5, sdo };
        let bytes = message.encode().expect("encode");
        assert!(bytes.len() <= MAILBOX_SIZE, "fits the mailbox");
        assert_eq!(Message::decode(&bytes).expect("decode"), message);
    }

    #[test]
    fn a_download_opens_with_its_size_and_first_bytes_then_segments() {
        let open = Sdo::InitiateDownload {
            index: 0x2000,
            subindex: 0,
            opening: Opening::Sized {
                size: 1000,
                data: vec![7; INITIATE_DATA],
            },
        };
        let bytes = Message {
            counter: 1,
            sdo: open.clone(),
        }
        .encode()
        .expect("encode");
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
        let segment = Message {
            counter: 1,
            sdo: Sdo::DownloadSegment {
                toggle: true,
                data: vec![9; SEGMENT_DATA],
                last: true,
            },
        };
        assert_eq!(segment.encode().expect("encode").len(), MAILBOX_SIZE);
        round(segment.sdo);
        round(Sdo::SegmentAccepted { toggle: true });
    }

    #[test]
    fn an_upload_is_the_mirror_and_an_abort_carries_its_reason() {
        round(Sdo::InitiateUpload {
            index: 0x1018,
            subindex: 1,
        });
        round(Sdo::UploadOpened {
            index: 0x1018,
            subindex: 1,
            opening: Opening::Sized {
                size: 4,
                data: vec![1, 2, 3, 4],
            },
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
        .encode()
        .expect("encode");
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
        let mut five = vec![0x0a, 0, 0, 0, 0, 0x03, 0, 0x20, 0xa0];
        five.extend_from_slice(&[0; 7]);
        assert!(Message::decode(&five).is_err(), "specifier 5");
        assert!(
            Message::decode(&[0x03, 0, 0, 0, 0, 0x03, 0, 0x20, 0x21]).is_err(),
            "short initiate"
        );
    }
}
