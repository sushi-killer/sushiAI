use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::Notify;
/// A cancellation flag that (unlike a bare `Notify`) keeps its state: any
/// number of callers can synchronously ask `is_cancelled()` at any point --
/// "check it between every step" -- without racing a lost wakeup, and
/// `cancelled()` still resolves immediately for anyone who starts waiting
/// after `cancel()` already ran.
#[derive(Clone)]
pub(crate) struct CancelToken(Arc<CancelInner>);

struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
    parent: Option<CancelToken>,
}

impl CancelToken {
    pub(crate) fn new() -> Self {
        CancelToken(Arc::new(CancelInner {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            parent: None,
        }))
    }

    pub(crate) fn child(&self) -> Self {
        CancelToken(Arc::new(CancelInner {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            parent: Some(self.clone()),
        }))
    }

    pub(crate) fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        let mut token = Some(self);
        while let Some(current) = token {
            if current.0.flag.load(Ordering::SeqCst) {
                return true;
            }
            token = current.0.parent.as_ref();
        }
        false
    }

    pub(crate) async fn cancelled(&self) {
        // Register interest *before* checking the flag, so a cancel() that
        // lands in between can't be missed (the documented tokio::Notify
        // pattern for exactly this race).
        let mut tokens = vec![self];
        while let Some(parent) = tokens.last().and_then(|t| t.0.parent.as_ref()) {
            tokens.push(parent);
        }
        let mut notified: Vec<_> = tokens
            .iter()
            .map(|t| Box::pin(t.0.notify.notified()))
            .collect();
        for wait in &mut notified {
            wait.as_mut().enable();
        }
        if self.is_cancelled() {
            return;
        }
        std::future::poll_fn(|cx| {
            for wait in &mut notified {
                if std::future::Future::poll(wait.as_mut(), cx).is_ready() {
                    return std::task::Poll::Ready(());
                }
            }
            std::task::Poll::Pending
        })
        .await;
    }
}
