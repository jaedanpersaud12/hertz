//! Everything that talks to the outside world: the Riot API (with its rate limits), Community Dragon,
//! and the crawler that fills the database.

pub mod cdragon;
pub mod client;
pub mod crawl;
pub mod limiter;

pub use client::{ACTIVATION_WAIT, KeyProblem, RiotClient, RiotError, http_client, init_tls, key_fingerprint};
pub use crawl::{CrawlOptions, Crawler};
