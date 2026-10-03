//! The small, generic API that generated dispatch code uses to talk to the
//! FFI runtime. Nothing here knows about customers, clients or any other WIT
//! type.

use crate::abi_types::*;
use crate::handles::{self, Entry, ObjectEntry, KIND_OBJECT, KIND_STREAM};
use crate::stream::StreamSlot;
use crate::wire::{WireError, Writer};
use futures::StreamExt;
use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type ObjectRef = Arc<dyn Any + Send + Sync>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
/// What a core returns for a WIT `stream<T>`.
pub type AbiStream<T> = Pin<Box<dyn futures::Stream<Item = T> + Send + 'static>>;

/// ABI-level failures (never domain errors — those are encoded payloads).
#[derive(Debug)]
pub enum AbiError {
    InvalidHandle,
    InvalidArgument(String),
    UnknownMethod,
    Unsupported,
    Internal(String),
}

impl AbiError {
    pub fn status(&self) -> wabi_status {
        match self {
            AbiError::InvalidHandle => WABI_ERR_INVALID_HANDLE,
            AbiError::InvalidArgument(_) => WABI_ERR_INVALID_ARGUMENT,
            AbiError::UnknownMethod => WABI_ERR_UNKNOWN_METHOD,
            AbiError::Unsupported => WABI_ERR_UNSUPPORTED,
            AbiError::Internal(_) => WABI_ERR_INTERNAL,
        }
    }
    pub fn message(&self) -> String {
        match self {
            AbiError::InvalidHandle => "invalid handle".into(),
            AbiError::InvalidArgument(m) => format!("invalid argument: {m}"),
            AbiError::UnknownMethod => "unknown method id".into(),
            AbiError::Unsupported => "operation not supported for this method".into(),
            AbiError::Internal(m) => format!("internal error: {m}"),
        }
    }
}

impl From<WireError> for AbiError {
    fn from(e: WireError) -> Self {
        AbiError::InvalidArgument(e.0)
    }
}

pub fn insert_object<T: Send + Sync + 'static>(group: u32, obj: Arc<T>, on_drop: impl FnOnce() + Send + 'static) -> u64 {
    let h = handles::alloc(KIND_OBJECT);
    handles::insert(h, Entry::Object(ObjectEntry { group, obj, on_drop: Some(Box::new(on_drop)) }));
    h
}

/// Resolves the receiver of a method call to the core's concrete type.
pub fn object<T: Send + Sync + 'static>(o: Option<ObjectRef>) -> Result<Arc<T>, AbiError> {
    o.ok_or(AbiError::InvalidHandle)?.downcast::<T>().map_err(|_| AbiError::InvalidHandle)
}

/// Wraps a typed stream into a stream handle; items are encoded lazily, one
/// per `next()`.
pub fn insert_stream<T: Send + 'static>(s: AbiStream<T>, enc: fn(&T, &mut Writer)) -> u64 {
    let bytes = s.map(move |item| {
        let mut w = Writer::new();
        enc(&item, &mut w);
        w.into_vec()
    });
    let h = handles::alloc(KIND_STREAM);
    handles::insert(h, Entry::Stream(Arc::new(StreamSlot::new(Box::pin(bytes)))));
    h
}
