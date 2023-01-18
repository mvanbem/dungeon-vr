use std::cmp::Ordering;
use std::marker::PhantomData;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

use tokio::time::Instant;

#[derive(Clone, Copy)]
pub struct ServerMarker;
pub type ServerEpoch = LocalEpoch<ServerMarker>;
pub type ServerTime = LocalTime<ServerMarker>;
pub type ServerOffset = TimeOffset<ServerMarker, ServerMarker>;

#[derive(Clone, Copy)]
pub struct ClientMarker;
pub type ClientEpoch = LocalEpoch<ClientMarker>;
pub type ClientTime = LocalTime<ClientMarker>;
pub type ClientOffset = TimeOffset<ClientMarker, ClientMarker>;

pub type ClientTimeToServerTime = TimeOffset<ClientMarker, ServerMarker>;

#[derive(Debug)]
pub struct LocalEpoch<M> {
    instant: Instant,
    _phantom_m: PhantomData<M>,
}

impl<M> Clone for LocalEpoch<M> {
    fn clone(&self) -> Self {
        Self {
            instant: self.instant,
            _phantom_m: PhantomData,
        }
    }
}

impl<M> Copy for LocalEpoch<M> {}

impl<M> LocalEpoch<M> {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            _phantom_m: PhantomData,
        }
    }

    pub fn now(self) -> LocalTime<M> {
        LocalTime::from_nanos_since_epoch(
            (Instant::now() - self.instant)
                .as_nanos()
                .try_into()
                .unwrap(),
        )
    }

    pub fn instant(self) -> Instant {
        self.instant
    }
}

#[derive(Debug)]
pub struct LocalTime<M> {
    nanos: u64,
    _phantom_m: PhantomData<M>,
}

impl<M> LocalTime<M> {
    pub fn from_nanos_since_epoch(nanos: u64) -> Self {
        Self {
            nanos,
            _phantom_m: PhantomData,
        }
    }

    pub fn to_nanos_since_epoch(self) -> u64 {
        self.nanos
    }

    pub fn to_instant(self, epoch: LocalEpoch<M>) -> Instant {
        epoch.instant + std::time::Duration::from_micros(self.nanos)
    }
}

impl<M> Clone for LocalTime<M> {
    fn clone(&self) -> Self {
        Self {
            nanos: self.nanos,
            _phantom_m: PhantomData,
        }
    }
}

impl<M> Copy for LocalTime<M> {}

impl<From, To> Add<TimeOffset<From, To>> for LocalTime<From> {
    type Output = LocalTime<To>;

    fn add(self, rhs: TimeOffset<From, To>) -> LocalTime<To> {
        LocalTime::from_nanos_since_epoch(self.nanos.checked_add_signed(rhs.nanos).unwrap())
    }
}

impl<M> AddAssign<TimeOffset<M, M>> for LocalTime<M> {
    fn add_assign(&mut self, rhs: TimeOffset<M, M>) {
        self.nanos = self.nanos.checked_add_signed(rhs.nanos).unwrap();
    }
}

impl<From, To> Sub<LocalTime<From>> for LocalTime<To> {
    type Output = TimeOffset<From, To>;

    fn sub(self, rhs: LocalTime<From>) -> TimeOffset<From, To> {
        TimeOffset::from_nanos(
            (i64::try_from(self.nanos).unwrap())
                .checked_sub(i64::try_from(rhs.nanos).unwrap())
                .unwrap(),
        )
    }
}

impl<M> PartialEq for LocalTime<M> {
    fn eq(&self, other: &Self) -> bool {
        self.nanos == other.nanos
    }
}

impl<M> Eq for LocalTime<M> {}

impl<M> PartialOrd for LocalTime<M> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.nanos.partial_cmp(&other.nanos)
    }
}

impl<M> Ord for LocalTime<M> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nanos.cmp(&other.nanos)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeOffset<From, To> {
    nanos: i64,
    _phantom_from: PhantomData<From>,
    _phantom_to: PhantomData<To>,
}

impl<From, To> TimeOffset<From, To> {
    pub fn from_nanos(nanos: i64) -> Self {
        Self {
            nanos,
            _phantom_from: PhantomData,
            _phantom_to: PhantomData,
        }
    }

    pub fn to_nanos(self) -> i64 {
        self.nanos
    }

    pub fn invert(&self) -> TimeOffset<To, From> {
        TimeOffset::from_nanos(self.nanos.checked_neg().unwrap())
    }
}

impl<From, To> Clone for TimeOffset<From, To> {
    fn clone(&self) -> Self {
        Self {
            nanos: self.nanos,
            _phantom_from: PhantomData,
            _phantom_to: PhantomData,
        }
    }
}

impl<From, To> Copy for TimeOffset<From, To> {}

impl<A, B, C> Add<TimeOffset<B, C>> for TimeOffset<A, B> {
    type Output = TimeOffset<A, C>;

    fn add(self, rhs: TimeOffset<B, C>) -> TimeOffset<A, C> {
        TimeOffset::from_nanos(self.nanos.checked_add(rhs.nanos).unwrap())
    }
}

impl<M> AddAssign for TimeOffset<M, M> {
    fn add_assign(&mut self, rhs: Self) {
        self.nanos = self.nanos.checked_add(rhs.nanos).unwrap();
    }
}

impl<A, B, C> Sub<TimeOffset<C, B>> for TimeOffset<A, B> {
    type Output = TimeOffset<A, C>;

    fn sub(self, rhs: TimeOffset<C, B>) -> TimeOffset<A, C> {
        TimeOffset::from_nanos(self.nanos.checked_sub(rhs.nanos).unwrap())
    }
}

impl<M> SubAssign for TimeOffset<M, M> {
    fn sub_assign(&mut self, rhs: Self) {
        self.nanos = self.nanos.checked_sub(rhs.nanos).unwrap();
    }
}

impl<From, To> Mul<i64> for TimeOffset<From, To> {
    type Output = Self;

    fn mul(self, rhs: i64) -> Self {
        Self::from_nanos(self.nanos * rhs)
    }
}

impl<From, To> MulAssign<i64> for TimeOffset<From, To> {
    fn mul_assign(&mut self, rhs: i64) {
        self.nanos *= rhs;
    }
}

impl<From, To> Div<i64> for TimeOffset<From, To> {
    type Output = Self;

    fn div(self, rhs: i64) -> Self {
        Self::from_nanos(self.nanos / rhs)
    }
}

impl<From, To> DivAssign<i64> for TimeOffset<From, To> {
    fn div_assign(&mut self, rhs: i64) {
        self.nanos /= rhs;
    }
}
