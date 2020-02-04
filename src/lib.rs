#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

use core::{
    future::Future,
    num::{
        NonZeroI128, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroIsize, NonZeroU128,
        NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8, NonZeroUsize,
    },
    sync::atomic::{
        AtomicBool, AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16, AtomicU32,
        AtomicU64, AtomicU8, AtomicUsize,
    },
    time::Duration,
};
use core_futures_io::{AsyncRead, AsyncWrite};
use futures::{
    future::{ready, Map, Ready},
    stream::{once, Forward, Once, StreamFuture},
    FutureExt, Sink, StreamExt,
};
use protocol::{
    format::{ByteFormat, ItemFormat},
    Bottom, Channels, Format, Protocol,
};
use serde::{de::DeserializeOwned, Serialize};

#[macro_export]
macro_rules! Serde {
    ($item:item) => {
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        $item
    };
}

#[derive(Debug)]
pub struct Insufficient;

pub trait Serializer<T: Serialize + DeserializeOwned> {
    type Serialize: Future<Output = Result<Self::Representation, Self::SerializeError>>;
    type SerializeError;
    type Deserialize: Future<Output = Result<T, Self::DeserializeError>>;
    type DeserializeError;
    type Representation;

    fn serialize(&mut self, item: T) -> Self::Serialize;

    fn deserialize(&mut self, item: Self::Representation) -> Self::Deserialize;
}

pub trait ByteSerializer<T: Serialize + DeserializeOwned>: Serializer<T> {
    type Serialize: Future<Output = Result<(), <Self as ByteSerializer<T>>::SerializeError>>;
    type SerializeError;
    type Deserialize: Future<Output = Result<T, <Self as ByteSerializer<T>>::DeserializeError>>;
    type DeserializeError;

    fn serialize<W: AsyncWrite>(
        &mut self,
        writer: &mut W,
        item: T,
    ) -> <Self as ByteSerializer<T>>::Serialize;

    fn deserialize<R: AsyncRead>(
        &mut self,
        reader: &mut R,
    ) -> <Self as ByteSerializer<T>>::Deserialize;
}

pub struct Serde<T>(T);

impl<T: Serializer<U>, U: Serialize + DeserializeOwned> Format<U> for Serde<T> {}

impl<T: Serializer<U>, U: Serialize + DeserializeOwned> ItemFormat<U> for Serde<T> {
    type Representation = T::Representation;
    type SerializeError = T::SerializeError;
    type Serialize = T::Serialize;
    type DeserializeError = T::DeserializeError;
    type Deserialize = T::Deserialize;

    fn serialize(&mut self, item: U) -> Self::Serialize {
        self.0.serialize(item)
    }

    fn deserialize(&mut self, item: T::Representation) -> Self::Deserialize {
        self.0.deserialize(item)
    }
}

impl<T: ByteSerializer<U>, U: Serialize + DeserializeOwned> ByteFormat<U> for Serde<T> {
    type SerializeError = <T as ByteSerializer<U>>::SerializeError;
    type Serialize = <T as ByteSerializer<U>>::Serialize;
    type DeserializeError = <T as ByteSerializer<U>>::DeserializeError;
    type Deserialize = <T as ByteSerializer<U>>::Deserialize;

    fn serialize<W: AsyncWrite>(&mut self, writer: &mut W, item: U) -> Self::Serialize {
        ByteSerializer::serialize(&mut self.0, writer, item)
    }

    fn deserialize<R: AsyncRead>(&mut self, reader: &mut R) -> Self::Deserialize {
        ByteSerializer::deserialize(&mut self.0, reader)
    }
}

// impl<T: Serializer> Format<Bottom> for Serde<T> {
//     type Representation = T::Representation;
//     type SerializeError = T::SerializeError;
//     type Serialize = Ready<Result<T::Representation, T::SerializeError>>;
//     type DeserializeError = T::DeserializeError;
//     type Deserialize = Ready<Result<Bottom, T::DeserializeError>>;

//     fn serialize(&mut self, _: Bottom) -> Self::Serialize {
//         panic!("attempted to serialize bottom type")
//     }

//     fn deserialize(&mut self, _: T::Representation) -> Self::Deserialize {
//         panic!("attempted to deserialize bottom type")
//     }
// }

macro_rules! flat {
    ( $( $x:ty ),* ) => {
        $(
            impl<T, C: Channels<$x, Bottom>> Protocol<Serde<T>, C> for $x
            where
                C::Unravel: Unpin,
                C::Coalesce: Unpin
            {
                type Unravel = $x;
                type UnravelError = <C::Unravel as Sink<$x>>::Error;
                type UnravelFuture =
                    Forward<Once<Ready<Result<$x, <C::Unravel as Sink<$x>>::Error>>>, C::Unravel>;
                type Coalesce = Bottom;
                type CoalesceError = Insufficient;
                type CoalesceFuture =
                    Map<StreamFuture<C::Coalesce>, fn((Option<$x>, C::Coalesce)) -> Result<$x, Insufficient>>;

                fn unravel(self, channel: C::Unravel) -> Self::UnravelFuture {
                    once(ready(Ok(self))).forward(channel)
                }

                fn coalesce(channel: C::Coalesce) -> Self::CoalesceFuture {
                    fn map<St>(next: (Option<$x>, St)) -> Result<$x, Insufficient> {
                        next.0.ok_or(Insufficient)
                    }
                    channel.into_future().map(map::<C::Coalesce>)
                }
            }
        )*
    };
    ( #[doc = $doc:literal] $( $x:ty ),* ) => {
        $(
            #[doc = $doc]
            flat!($x);
        )*
    };
}

flat! {
    bool, char, f32, f64, Duration,
    usize, u8, u16, u32, u64, u128,
    isize, i8, i16, i32, i64, i128,
    AtomicBool,
    AtomicIsize, AtomicI8, AtomicI16, AtomicI32, AtomicI64,
    AtomicUsize, AtomicU8, AtomicU16, AtomicU32, AtomicU64,
    NonZeroIsize, NonZeroI8, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI128,
    NonZeroUsize, NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128
}

#[cfg(feature = "std")]
mod std {
    use super::*;
    use ::std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
        time::SystemTime,
    };
    flat! {
        /// *This requires the `alloc` feature, which is enabled by default*
        IpAddr, Ipv4Addr, Ipv6Addr,
        SocketAddr, SocketAddrV4, SocketAddrV6,
        SystemTime
    }
}

#[cfg(feature = "alloc")]
mod _alloc {
    use super::*;
    use ::alloc::string::String;
    flat! {
        // *This requires the `alloc` feature, which is enabled by default*
        String
    }
}
