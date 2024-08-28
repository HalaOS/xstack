# The better libp2p protocol stack for rust

[![CI](https://github.com/HalaOS/xstack/actions/workflows/ci.yaml/badge.svg)](https://github.com/HalaOS/xstack/actions/workflows/ci.yaml)
[![Release](https://github.com/HalaOS/xstack/actions/workflows/release.yaml/badge.svg)](https://github.com/HalaOS/xstack/actions/workflows/release.yaml)
[!["Crates.io version"](https://img.shields.io/crates/v/xstack.svg)](https://crates.io/crates/xstack)

**XSTACK** was developed with the primary goal of making it less difficult to use the [`libp2p`](https://libp2p.io/) network stack in rust,
for this purpose, the API of XSTACK has been designed to closely resemble the std::net library.

## Usage

Please refer to the [`docs`](https://docs.rs/xstack) for more details.

## Status

| crate       |  version                                                                      |  type           | status      |
| ----------- | ----------------------------------------------------------------------------- | --------------- | ----------- |
| Core        | [![docs.rs (with version)](https://img.shields.io/docsrs/xstack/0.2.15)](https://docs.rs/xstack/latest/xstack/)        | framework       | stable      |
| Quic        | [![docs.rs (with version)](https://img.shields.io/docsrs/xstack-quic/0.2.15)](https://docs.rs/xstack/latest/xstack-quic/)   | transport layer | stable      |
| Tcp         | [![docs.rs (with version)](https://img.shields.io/docsrs/xstack-tcp/0.2.15)](https://docs.rs/xstack/latest/xstack-tcp/)    | transport layer | stable      |
| dnsaddr     | [![docs.rs (with version)](https://img.shields.io/docsrs/xstack-dnsaddr/0.2.15)](https://docs.rs/xstack/latest/xstack-dnsaddr/)| transport layer | available   |
| Kad         | [![docs.rs (with version)](https://img.shields.io/docsrs/xstack-kad/0.2.15)](https://docs.rs/xstack/latest/xstack-kad/)    | protocol layer  | in progress |
