# xmip-core-transport-ethercat

EtherCAT transport: IEC 61158 Type 12 over raw Ethernet — datagrams with command, index, address, length and working counter, several to a frame, and a CoE mailbox transfer for a Stream larger than one; the SDO in the mailbox is the canopen technology's. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A send target is read by `net::Target` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net), the one reading of a URI every technology calls: scheme, authority, path and decoded query. Until 2026-09-28 it was read through the transport capability's `socket::target`, which split it on its first slash and left the query in the path.

A Send Location downloads to the object its `station`, `index` and `subindex` settings name, on the link its address names: it sends to the empty target (`Configured::own_target`, `transport::destination::OWN`), which it reads, through `transport::destination::named`, as that object. A target naming another still wins — `0x1002/0x2000/1` (ADR-0064, amendment 2026-11-10).

A `0x` number in a target is read by `codec::hex::prefixed_number` in [xmip-core-library-codec](https://github.com/IlleNilsson/xmip-core-library-codec), which refuses a sign; until 2026-09-28 it was read with `from_str_radix`, which took `0x+7e8`.

The link is ethernet's (`ethernet::open_link`): an interface's raw socket on Linux, as the root's service test proves a Send Location on a veth pair whose far end is a ring of this crate's slaves (2026-11-25). A slave says when nothing waits in its transmit mailbox for the master to read (`Slave::idle`), so a ring at the far end of a wire knows a download is answered whole.

## Acknowledgement

A receive is a CoE upload of the object through the slave's mailbox, which
consumes nothing at the slave. Its verdict therefore has nothing to tell the
slave, whichever it is: a receive cycle that did not complete loses nothing,
and the next upload reads the object again. The object's bytes arrive whole.

## The declaration

What a Location of this technology is configured with is two `const`s in its `declaration` module, `src/declaration.rs`, beside the code, which `Configured` reads (ADR-0064, amendment 2026-11-26): `declaration::SETTINGS`, `source`, `station`, `index`, `subindex` and `timeout`, and `SCHEMES`, `ethercat`. The object a Stream travels as and the wait `timeout` defaults to, `STREAM_OBJECT` and `TIMEOUT`, are there too, and the code reads them from there; `STREAM_OBJECT` is at the crate's root as well. It is the same on every operating system, those without the kernel's raw socket included. Built with its `declaration` feature alone (`--no-default-features --features declaration`), the crate is those and nothing more: its code and every crate the code is built on — the ethernet technology it rides on and the canopen technology whose SDO it speaks among them — are behind the default feature. So the library the surfaces load can carry it without the code, for the language server and the desktop to suggest and check an `EtherCAT` Location's settings though no node in their process loaded the technology, once the gathering crate of open problem 48 links it. `test/Declaration.Test.ps1` in the estate holds that the declaration builds nothing the transport capability does not.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
