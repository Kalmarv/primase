//! Anvil engine containers. So far the forge archive's index ([`read::forge`]): a `.forge` file of version 27 opened
//! and its entries listed (offset, size and both ids as stored, with each entry's name record). Entry payloads are
//! located only, never decoded: their compression is not established.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod read;
