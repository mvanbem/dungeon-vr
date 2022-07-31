use std::fmt::{self, Debug, Formatter};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// A cancellation token.
///
/// Once set, always cancelled. Can be queried, or awaited on for cancellation.
///
/// This is clonable, and represents the same underlying cancellation state.
#[derive(Clone, Default)]
pub struct Token {
    inner: Arc<Inner>,
}

impl Token {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    pub fn cancel(&self) {
        self.inner.cancel()
    }

    pub async fn cancelled(&self) {
        self.inner.cancelled().await
    }

    pub fn guard(&self) -> Guard {
        Guard::new(self.clone())
    }
}

impl Debug for Token {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("Token")
            .field("is_cancelled", &self.is_cancelled())
            .finish()
    }
}

/// A cancellation token guard.
///
/// On drop, cancels the token.
#[derive(Clone)]
pub struct Guard {
    token: Token,
}

impl Guard {
    pub fn new(token: Token) -> Self {
        Self { token }
    }
}

impl Deref for Guard {
    type Target = Token;

    fn deref(&self) -> &Token {
        &self.token
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

#[derive(Default)]
struct Inner {
    is_cancelled: AtomicBool,
    cancelled_notification: Notify,
}

impl Inner {
    pub fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        if !self.is_cancelled.swap(true, Ordering::Release) {
            self.cancelled_notification.notify_waiters();
        }
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }

        // Add ourselves to waitlist before querying again. We're here because we observed
        // is_cancelled as false - if it becomes true we won't bother awaiting notified,
        // but if it remains false then we are on the waiter list before it becomes true.
        let notified = self.cancelled_notification.notified();
        if !self.is_cancelled() {
            notified.await
        }
    }
}
