#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

use core::{
    marker::PhantomData,
    num::{
        NonZeroI128, NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroIsize, NonZeroU128,
        NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8, NonZeroUsize,
    },
    pin::Pin,
    sync::atomic::{
        AtomicBool, AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16, AtomicU32,
        AtomicU64, AtomicU8, AtomicUsize,
    },
    task::{Context, Poll},
    time::Duration,
};
use futures::{
    future::{ready, Map, Ready},
    ready,
    stream::{once, Forward, IntoStream, Once, StreamFuture},
    FutureExt, Sink, Stream, StreamExt, TryStream, TryStreamExt,
};
use protocol::{format::ItemFormat, Bottom, Channels, Format, Protocol};
use serde::{de::DeserializeOwned, Serialize};

#[macro_export]
macro_rules! Serde {
    ($item:item) => {
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        $item
    };
}

#[derive(Debug)]
pub enum SerdeError<T> {
    Insufficient,
    Stream(T),
}

pub trait Serializer<T: Serialize + DeserializeOwned> {
    type SerializeError;
    type DeserializeError;
    type Representation;

    fn serialize(&mut self, item: T) -> Result<Self::Representation, Self::SerializeError>;

    fn deserialize(&mut self, item: Self::Representation) -> Result<T, Self::DeserializeError>;
}

pub struct Serde<T>(T);

impl<T> Serde<T> {
    pub fn new(serializer: T) -> Self {
        Serde(serializer)
    }
}

pub struct SerdeOutput<
    U: Serialize + DeserializeOwned,
    T: Serializer<U>,
    S: Sink<T::Representation> + TryStream<Ok = T::Representation>,
> {
    serializer: T,
    channel: IntoStream<S>,
    data: PhantomData<U>,
}

impl<
        U: Unpin + Serialize + DeserializeOwned,
        T: Unpin + Serializer<U>,
        S: Unpin + Sink<T::Representation> + TryStream<Ok = T::Representation>,
    > Stream for SerdeOutput<U, T, S>
{
    type Item = Result<U, ItemError<T::DeserializeError, <S as TryStream>::Error>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let item = ready!(Pin::new(&mut this.channel).poll_next(cx));
        if let Some(item) = item {
            let item = item.map_err(ItemError::Stream)?;
            Poll::Ready(Some(
                this.serializer.deserialize(item).map_err(ItemError::Serde),
            ))
        } else {
            return Poll::Ready(None);
        }
    }
}

#[derive(Debug)]
pub enum ItemError<T, E> {
    Serde(T),
    Stream(E),
}

impl<
        U: Unpin + Serialize + DeserializeOwned,
        T: Unpin + Serializer<U>,
        S: Unpin + Sink<T::Representation> + TryStream<Ok = T::Representation>,
    > Sink<U> for SerdeOutput<U, T, S>
{
    type Error = ItemError<T::SerializeError, <S as Sink<T::Representation>>::Error>;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.channel)
            .poll_ready(cx)
            .map_err(ItemError::Stream)
    }

    fn start_send(mut self: Pin<&mut Self>, item: U) -> Result<(), Self::Error> {
        let this = &mut *self;
        let serialized = this.serializer.serialize(item).map_err(ItemError::Serde)?;
        Pin::new(&mut this.channel)
            .start_send(serialized)
            .map_err(ItemError::Stream)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.channel)
            .poll_flush(cx)
            .map_err(ItemError::Stream)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.channel)
            .poll_close(cx)
            .map_err(ItemError::Stream)
    }
}

impl<T: Serializer<U>, U: Serialize + DeserializeOwned> Format<U> for Serde<T> {}

impl<
        T: Unpin + Serializer<U>,
        U: Unpin + Serialize + DeserializeOwned,
        S: Unpin + Sink<T::Representation> + TryStream<Ok = T::Representation>,
    > ItemFormat<U, S> for Serde<T>
{
    type Representation = T::Representation;
    type Output = SerdeOutput<U, T, S>;

    fn wire(self, channel: S) -> Self::Output {
        SerdeOutput {
            serializer: self.0,
            channel: channel.into_stream(),
            data: PhantomData,
        }
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
                C::Coalesce: Unpin,
            {
                type Unravel = $x;
                type UnravelError = <C::Unravel as Sink<$x>>::Error;
                type UnravelFuture =
                    Forward<Once<Ready<Result<$x, <C::Unravel as Sink<$x>>::Error>>>, C::Unravel>;
                type Coalesce = Bottom;
                type CoalesceError = SerdeError<<C::Coalesce as TryStream>::Error>;
                type CoalesceFuture = Map<
                    StreamFuture<IntoStream<C::Coalesce>>,
                    fn(
                        (
                            Option<Result<<C::Coalesce as TryStream>::Ok, <C::Coalesce as TryStream>::Error>>,
                            IntoStream<C::Coalesce>,
                        ),
                    ) -> Result<$x, SerdeError<<C::Coalesce as TryStream>::Error>>,
                >;

                fn unravel(self, channel: C::Unravel) -> Self::UnravelFuture {
                    once(ready(Ok(self))).forward(channel)
                }

                fn coalesce(channel: C::Coalesce) -> Self::CoalesceFuture {
                    fn map<St, U>(next: (Option<Result<$x, U>>, St)) -> Result<$x, SerdeError<U>> {
                        next.0
                            .ok_or(SerdeError::Insufficient)?
                            .map_err(SerdeError::Stream)
                    }
                    channel
                        .into_stream()
                        .into_future()
                        .map(map::<IntoStream<C::Coalesce>, <C::Coalesce as TryStream>::Error>)
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
