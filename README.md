# The better libp2p protocol stack for rust

[![CI](https://github.com/HalaOS/xstack/actions/workflows/ci.yaml/badge.svg)](https://github.com/HalaOS/xstack/actions/workflows/ci.yaml)
[![Release](https://github.com/HalaOS/xstack/actions/workflows/release.yaml/badge.svg)](https://github.com/HalaOS/xstack/actions/workflows/release.yaml)
[!["Crates.io version"](https://img.shields.io/crates/v/xstack.svg)](https://crates.io/crates/xstack)
[!["docs.rs docs"](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/xstack)

**XSTACK** was developed with the primary goal of making it less difficult to use the [`libp2p`](https://libp2p.io/) network stack in rust,
for this purpose, the API of XSTACK has been designed to closely resemble the std::net library.

## Usage

Please refer to the [`docs`](https://docs.rs/xstack) for more details.

## Status

| crate      | repository |type|status|
| ----------- | ------------------------------- | --------------- | ----------- |
| Core        | [`xstack`](./crates/xstack/)    | framework       | stable      |
| Quic        | [`quic`](./crates/quic/)        | transport layer | stable      |
| Tcp         | [`tcp`](./crates/tcp/)          | transport layer | stable      |
| dnsaddr     | [`dnsaddr`](./crates/dnsaddr/)  | transport layer | available   |
| Kad         | [`kad`](./crates/kad/)          | protocol layer  | in progress |
