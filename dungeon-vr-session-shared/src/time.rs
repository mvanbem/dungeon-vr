use std::marker::PhantomData;
use std::ops::{Add, AddAssign, Sub};
use std::time::Duration;

use tokio::time::Instant;

pub struct ServerMarker;
pub type ServerEpoch = LocalEpoch<ServerMarker>;
pub type ServerTime = LocalTime<ServerMarker>;

pub struct ClientMarker;
pub type ClientEpoch = LocalEpoch<ClientMarker>;
pub type ClientTime = LocalTime<ClientMarker>;

pub struct LocalEpoch<M: 'static> {
    instant: Instant,
    _phantom_m: PhantomData<M>,
}

impl<M> LocalEpoch<M> {
    pub fn new() -> Self {
        Self {
            instant: Instant::now(),
            _phantom_m: PhantomData,
        }
    }

    pub fn now(self) -> LocalTime<M> {
        LocalTime {
            micros: (Instant::now() - self.instant)
                .as_micros()
                .try_into()
                .unwrap(),
            _phantom_m: PhantomData,
        }
    }
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

#[derive(Debug, PartialEq, Eq)]
pub struct LocalTime<M: 'static> {
    micros: u64,
    _phantom_m: PhantomData<M>,
}

impl<M> LocalTime<M> {
    pub fn from_micros_since_epoch(micros: u64) -> Self {
        Self {
            micros,
            _phantom_m: PhantomData,
        }
    }

    pub fn to_micros_since_epoch(self) -> u64 {
        self.micros
    }
}

impl<M> Clone for LocalTime<M> {
    fn clone(&self) -> Self {
        Self {
            micros: self.micros,
            _phantom_m: PhantomData,
        }
    }
}

impl<M> Copy for LocalTime<M> {}

impl<M> Add<Duration> for LocalTime<M> {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self {
        Self {
            micros: self
                .micros
                .checked_add(rhs.as_micros().try_into().unwrap())
                .unwrap(),
            _phantom_m: PhantomData,
        }
    }
}

impl<M> AddAssign<Duration> for LocalTime<M> {
    fn add_assign(&mut self, rhs: Duration) {
        self.micros = self
            .micros
            .checked_add(rhs.as_micros().try_into().unwrap())
            .unwrap();
    }
}

impl<M> Sub for LocalTime<M> {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Duration {
        Duration::from_micros(self.micros.checked_sub(rhs.micros).unwrap())
    }
}

impl<M> Sub<Duration> for LocalTime<M> {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self {
        Self {
            micros: self
                .micros
                .checked_sub(rhs.as_micros().try_into().unwrap())
                .unwrap(),
            _phantom_m: PhantomData,
        }
    }
}
