use std::cmp::Ordering;
use std::iter::Sum;
use std::marker::PhantomData;
use std::num::TryFromIntError;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ServerMarker;
pub type ServerTokioEpoch = TokioEpoch<ServerMarker>;
pub type ServerTime = NanoTime<ServerMarker>;

#[derive(Clone, Copy, Debug)]
pub struct ClientMarker;
pub type ClientTokioEpoch = TokioEpoch<ClientMarker>;
pub type ClientTime = NanoTime<ClientMarker>;

#[derive(Debug)]
pub struct TokioEpoch<M> {
    instant: Instant,
    _phantom_m: PhantomData<M>,
}

impl<M> Clone for TokioEpoch<M> {
    fn clone(&self) -> Self {
        Self {
            instant: self.instant,
            _phantom_m: PhantomData,
        }
    }
}

impl<M> Copy for TokioEpoch<M> {}

impl<M> TokioEpoch<M> {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            _phantom_m: PhantomData,
        }
    }

    pub fn now(self) -> NanoTime<M> {
        NanoTime::from_nanos_since_epoch(
            (Instant::now() - self.instant)
                .as_nanos()
                .try_into()
                .unwrap(),
        )
    }

    pub fn instant(self) -> Instant {
        self.instant
    }

    pub fn instant_at(self, time: NanoTime<M>) -> Instant {
        self.instant
            + std::time::Duration::from_nanos(time.as_nanos_since_epoch().try_into().unwrap())
    }
}

#[derive(Debug)]
pub struct NanoTime<M> {
    nanos: i64,
    _phantom_m: PhantomData<M>,
}

impl<M> NanoTime<M> {
    pub fn from_nanos_since_epoch(nanos: i64) -> Self {
        Self {
            nanos,
            _phantom_m: PhantomData,
        }
    }

    pub fn as_nanos_since_epoch(self) -> i64 {
        self.nanos
    }
}

impl<M> Clone for NanoTime<M> {
    fn clone(&self) -> Self {
        Self {
            nanos: self.nanos,
            _phantom_m: PhantomData,
        }
    }
}

impl<M> Copy for NanoTime<M> {}

impl<M> Add<NanoDuration> for NanoTime<M> {
    type Output = Self;

    fn add(self, rhs: NanoDuration) -> Self {
        NanoTime::from_nanos_since_epoch(self.nanos.checked_add(rhs.nanos).unwrap())
    }
}

impl<M> AddAssign<NanoDuration> for NanoTime<M> {
    fn add_assign(&mut self, rhs: NanoDuration) {
        self.nanos = self.nanos.checked_add(rhs.nanos).unwrap();
    }
}

impl<M> Sub<NanoTime<M>> for NanoTime<M> {
    type Output = NanoDuration;

    fn sub(self, rhs: Self) -> NanoDuration {
        NanoDuration::from_nanos(self.nanos.checked_sub(rhs.nanos).unwrap())
    }
}

impl<M> PartialEq for NanoTime<M> {
    fn eq(&self, other: &Self) -> bool {
        self.nanos == other.nanos
    }
}

impl<M> Eq for NanoTime<M> {}

impl<M> PartialOrd for NanoTime<M> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.nanos.partial_cmp(&other.nanos)
    }
}

impl<M> Ord for NanoTime<M> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nanos.cmp(&other.nanos)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NanoDuration {
    nanos: i64,
}

impl NanoDuration {
    pub const fn from_nanos(nanos: i64) -> Self {
        Self { nanos }
    }

    pub fn from_secs_f32(secs: f32) -> Self {
        let nanos = (secs * 1e9).round();
        // TODO: This check seems imprecise.
        assert!(nanos >= i64::MIN as f32 && nanos <= i64::MAX as f32);
        Self::from_nanos(nanos as i64)
    }

    pub fn from_secs_f64(secs: f64) -> Self {
        let nanos = (secs * 1e9).round();
        // TODO: This check seems imprecise.
        assert!(nanos >= i64::MIN as f64 && nanos <= i64::MAX as f64);
        Self::from_nanos(nanos as i64)
    }

    pub const fn as_nanos(self) -> i64 {
        self.nanos
    }

    pub fn as_secs_f32(self) -> f32 {
        self.nanos as f32 * 1e-9
    }

    pub fn as_secs_f64(self) -> f64 {
        self.nanos as f64 * 1e-9
    }
}

impl Clone for NanoDuration {
    fn clone(&self) -> Self {
        Self { nanos: self.nanos }
    }
}

impl Copy for NanoDuration {}

impl TryFrom<std::time::Duration> for NanoDuration {
    type Error = TryFromIntError;

    fn try_from(value: std::time::Duration) -> Result<Self, TryFromIntError> {
        Ok(Self::from_nanos(value.as_nanos().try_into()?))
    }
}

impl TryFrom<NanoDuration> for std::time::Duration {
    type Error = TryFromIntError;

    fn try_from(value: NanoDuration) -> Result<Self, TryFromIntError> {
        Ok(std::time::Duration::from_nanos(value.nanos.try_into()?))
    }
}

impl Add for NanoDuration {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::from_nanos(self.nanos.checked_add(rhs.nanos).unwrap())
    }
}

impl AddAssign for NanoDuration {
    fn add_assign(&mut self, rhs: Self) {
        self.nanos = self.nanos.checked_add(rhs.nanos).unwrap();
    }
}

impl Sub for NanoDuration {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::from_nanos(self.nanos.checked_sub(rhs.nanos).unwrap())
    }
}

impl SubAssign for NanoDuration {
    fn sub_assign(&mut self, rhs: Self) {
        self.nanos = self.nanos.checked_sub(rhs.nanos).unwrap();
    }
}

impl Mul<i64> for NanoDuration {
    type Output = Self;

    fn mul(self, rhs: i64) -> Self {
        Self::from_nanos(self.nanos * rhs)
    }
}

impl MulAssign<i64> for NanoDuration {
    fn mul_assign(&mut self, rhs: i64) {
        self.nanos *= rhs;
    }
}

impl Div<i64> for NanoDuration {
    type Output = Self;

    fn div(self, rhs: i64) -> Self {
        Self::from_nanos(self.nanos / rhs)
    }
}

impl Div for NanoDuration {
    type Output = i64;

    fn div(self, rhs: Self) -> i64 {
        self.nanos / rhs.nanos
    }
}

impl DivAssign<i64> for NanoDuration {
    fn div_assign(&mut self, rhs: i64) {
        self.nanos /= rhs;
    }
}

impl Sum for NanoDuration {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::from_nanos(0), |acc, x| acc + x)
    }
}
