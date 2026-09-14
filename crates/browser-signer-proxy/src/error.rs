// Copyright (c) 2022-2023 Yuki Kishimoto
// Copyright (c) 2023-2025 Rust Nostr Developers
// Distributed under the MIT software license

//! Error types for the browser signer proxy.

opaquerr::define_kind! {
    /// Nostr browser signer proxy error kind.
    pub ErrorKind {
        /// Nostr protocol error.
        Protocol => "nostr protocol error",
        /// I/O error.
        IO => "I/O error",
        /// JSON error.
        Json => "JSON error",
        /// The operation timed out.
        Timeout => "timeout",
        /// The operation cannot be completed in the current state.
        State => "invalid state",
        /// The signer returned an error for the requested operation.
        Rejected => "operation rejected",
        /// Anything not covered by the stable categories above.
        Other => "other error",
    }
}

opaquerr::define_error! {
    /// Nostr browser signer proxy error.
    pub Error(ErrorKind)

    from {
        nostr::error::Error => ErrorKind::Protocol,
        std::io::Error => ErrorKind::IO,
        serde_json::Error => ErrorKind::Json,
    }
}

impl Error {
    pub(crate) fn generic<S>(message: S) -> Self
    where
        S: Into<String>,
    {
        Self::new(ErrorKind::Rejected, message.into())
    }

    pub(crate) fn timeout() -> Self {
        Self::simple(ErrorKind::Timeout)
    }

    pub(crate) fn shutdown() -> Self {
        Self::with_static_message(ErrorKind::State, "server is shutdown")
    }
}
