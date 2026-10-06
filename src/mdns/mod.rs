// src/mdns/mod.rs
//!
//! # mDNS Service Discovery
//!
//! mDNS/Bonjour service discovery for local network node discovery.
//! Uses .local domain names for zero-configuration networking.

pub mod discovery;

pub use discovery::{
    MdnsService, MdnsAnnouncer, MdnsBrowser, DiscoveredNode,
    YANDI_SERVICE_TYPE, YANDI_ADMIN_TYPE
};
