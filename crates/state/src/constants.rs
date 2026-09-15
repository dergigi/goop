/// Client name (Application name)
pub const CLIENT_NAME: &str = "Goop";

/// App ID
pub const APP_ID: &str = "com.dergigi.goop";

/// Keyring name
pub const MASTER_KEYRING: &str = "Goop Master Key";
pub const USER_KEYRING: &str = "Goop User Credential";

/// Default timeout for subscription
pub const TIMEOUT: u64 = 2;

/// Default image cache size
pub const IMAGE_CACHE_SIZE: usize = 20;

/// Default delay for searching
pub const FIND_DELAY: u64 = 600;

/// Default subscription id for user gift wrap events
pub const USER_GIFTWRAP: &str = "user-gift-wraps";

/// Default timeout for Nostr Connect
pub const NOSTR_CONNECT_TIMEOUT: u64 = 60;

/// Default Nostr Connect relay
pub const NOSTR_CONNECT_RELAY: &str = "wss://relay.nip46.com";

/// Default vertex relays
pub const WOT_RELAYS: [&str; 1] = ["wss://relay.vertexlab.io"];

/// Default search relays
pub const INDEXER_RELAYS: [&str; 2] = ["wss://indexer.coracle.social", "wss://user.kindpag.es"];

/// Default bootstrap relays
pub const BOOTSTRAP_RELAYS: [&str; 4] = [
    "wss://relay.ditto.pub",
    "wss://relay.primal.net",
    "wss://relay.nostr.net",
    "wss://profiles.nostr1.com",
];
