//! Strongly-typed identifiers, so an `AccountId` can never be passed where a
//! `UserId` is expected.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type)]
        #[serde(transparent)]
        #[sqlx(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    )*};
}

id_type!(UserId, SessionId, AccountId, OrderId, TradeId, EntryId);
