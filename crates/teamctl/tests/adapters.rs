//! Offline external adapter host/lifecycle/terminal contracts.
#![cfg(unix)]

#[path = "adapters/fixture.rs"]
mod fixture;
#[path = "adapters/identify.rs"]
mod identify;
#[path = "adapters/lifecycle.rs"]
mod lifecycle;
#[path = "adapters/protocol.rs"]
mod protocol;
