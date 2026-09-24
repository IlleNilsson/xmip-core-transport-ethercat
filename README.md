# xmip-core-transport-ethercat

EtherCAT transport: IEC 61158 Type 12 over raw Ethernet — datagrams with command, index, address, length and working counter, several to a frame, and a CoE mailbox transfer for a Stream larger than one; the SDO in the mailbox is the canopen technology's. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
