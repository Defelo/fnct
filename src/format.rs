//! Formatters are used to serialize and deserialize data for storing it in a cache.

use std::fmt::Debug;

use serde::{de::DeserializeOwned, Serialize};

/// A formatter that can serialize data before storing it in a cache and deserialize it after
/// retrieving it from the cache.
pub trait Formatter: Debug {
    /// The error to return if serialization or deserialization fails.
    type Error;
    /// The type of serialized data.
    type Serialized;

    /// Serialize a `T` and return a [`Self::Serialized`].
    fn serialize<T: Serialize>(&self, value: &T) -> Result<Self::Serialized, Self::Error>;
    /// Deserialize a [`Self::Serialized`] and return a `T`.
    fn deserialize<T: DeserializeOwned>(&self, value: &Self::Serialized) -> Result<T, Self::Error>;
}

/// A formatter using the [`postcard`] crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct PostcardFormatter;

impl Formatter for PostcardFormatter {
    type Error = postcard::Error;
    type Serialized = Vec<u8>;

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Self::Serialized, Self::Error> {
        postcard::to_stdvec(value)
    }

    fn deserialize<T: DeserializeOwned>(&self, value: &Self::Serialized) -> Result<T, Self::Error> {
        postcard::from_bytes(value)
    }
}

/// A formatter using the [`serde_json`] crate.
#[cfg(feature = "serde_json")]
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonFormatter;

#[cfg(feature = "serde_json")]
impl Formatter for JsonFormatter {
    type Error = serde_json::Error;
    type Serialized = String;

    fn serialize<T: Serialize>(&self, value: &T) -> Result<Self::Serialized, Self::Error> {
        serde_json::to_string(value)
    }

    fn deserialize<T: DeserializeOwned>(&self, value: &Self::Serialized) -> Result<T, Self::Error> {
        serde_json::from_str(value)
    }
}
