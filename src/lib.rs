//! `proxyforge` — a fast, concurrent proxy checker.
//!
//! The crate exposes the building blocks of the command line tool so they can
//! be reused and tested independently:
//!
//! * [`proxy`] parses proxy endpoints,
//! * [`transport`] performs HTTP requests through HTTP, SOCKS4 and SOCKS5
//!   proxies,
//! * [`checker`] runs checks concurrently,
//! * [`anonymity`] classifies what the judge can see,
//! * [`output`] renders the results.
//!
//! Most users will want the `proxyforge` binary instead.

pub mod anonymity;
pub mod checker;
pub mod cli;
pub mod input;
pub mod output;
pub mod proxy;
pub mod report;
pub mod transport;
