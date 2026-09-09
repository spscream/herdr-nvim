//! Hold `herdr-sidebar`'s ensure lock for the length of a layout maneuver.
//!
//! `maneuver::open` is not atomic. It parks every non-anchor pane on a
//! temporary tab, creates the nvim pane, and only then puts the parked panes
//! back. For those few hundred milliseconds the tab has no file explorer in
//! it, and the parking tab is a brand-new tab that has none either.
//!
//! herdr-sidebar docks an explorer into any tab an event says has none. It
//! listens on `tab.created` and on `pane.focused`, and the maneuver raises
//! both: it creates the parking tab, and it creates the nvim pane with focus.
//! A hook run that lands inside the window does one of two things. It docks a
//! second explorer into the tab, and the user's own explorer returns from the
//! parking tab a moment later, so the tab ends with two. Or it docks one into
//! the parking tab, which the maneuver does not know about and therefore never
//! empties, so an orphan tab is left behind holding an explorer and the
//! maneuver's placeholder shell.
//!
//! herdr-sidebar already serialises its own runs through a lock directory, and
//! a run that cannot take the lock skips itself and waits for the next event.
//! Taking that same lock for the length of the maneuver is the whole fix: the
//! hook keeps its own rule, and simply does not look while the layout is in
//! pieces.
//!
//! One behaviour changes with it. A tab that has no explorer at all, and that
//! would have had one docked by the `pane.focused` event the maneuver raises,
//! does not get one now. herdr-sidebar re-fires on the next focus event, so
//! the explorer arrives a moment later instead of in the middle of the move.
//!
//! Every failure in this module is silent and non-fatal. The lock belongs to
//! another plugin. A maneuver that cannot take it behaves exactly as it did
//! before this module existed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// herdr-sidebar builds the same path from `std::env::temp_dir()`.
pub(crate) fn lock_dir() -> PathBuf {
    std::env::temp_dir().join("herdr-sidebar-ensure.lock")
}

/// The age at which herdr-sidebar treats a lock as left by a crashed run and
/// breaks it. Kept identical here so the two sides never disagree about who
/// owns a lock.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// How long to wait for a dock that is already in flight. herdr-sidebar holds
/// the lock until its explorer's interface reports itself alive, which it
/// measures in seconds, so waiting that out would stall the click the user is
/// waiting on. A short wait covers ordinary contention. After it the maneuver
/// goes ahead unguarded, which is what it always did.
const WAIT: Duration = Duration::from_millis(1200);
const POLL: Duration = Duration::from_millis(100);

/// Releases the lock on drop, and only if this process took it.
pub struct Guard(Option<PathBuf>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir(&dir);
        }
    }
}

/// Set by a caller that already holds the lock itself, so this process does
/// not wait out its own guard. herdr-sidebar sets it when it drives an
/// `open-file` from inside its own locked section.
const ALREADY_HELD: &str = "HERDR_SIDEBAR_ENSURE_LOCK_HELD";

pub fn hold() -> Guard {
    if std::env::var_os(ALREADY_HELD).is_some() {
        return Guard(None);
    }
    hold_in(lock_dir(), WAIT, STALE_AFTER)
}

fn hold_in(dir: PathBuf, wait: Duration, stale_after: Duration) -> Guard {
    let deadline = Instant::now() + wait;
    loop {
        if std::fs::create_dir(&dir).is_ok() {
            return Guard(Some(dir));
        }
        if is_stale(&dir, stale_after) {
            let _ = std::fs::remove_dir_all(&dir);
            if std::fs::create_dir(&dir).is_ok() {
                return Guard(Some(dir));
            }
        }
        if Instant::now() >= deadline {
            return Guard(None);
        }
        std::thread::sleep(POLL);
    }
}

fn is_stale(dir: &Path, stale_after: Duration) -> bool {
    std::fs::metadata(dir)
        .and_then(|meta| meta.created().or_else(|_| meta.modified()))
        .ok()
        .and_then(|created| created.elapsed().ok())
        .is_some_and(|age| age > stale_after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "herdr-nvim-lock-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn holding_takes_the_lock_and_dropping_releases_it() {
        let dir = scratch("take");
        {
            let guard = hold_in(dir.clone(), Duration::ZERO, STALE_AFTER);
            assert!(guard.0.is_some(), "an unheld lock must be taken");
            assert!(dir.is_dir(), "the lock directory must exist while held");
        }
        assert!(!dir.exists(), "dropping the guard must release the lock");
    }

    #[test]
    fn a_lock_someone_else_holds_is_left_alone() {
        let dir = scratch("contended");
        std::fs::create_dir_all(&dir).unwrap();
        {
            let guard = hold_in(dir.clone(), Duration::ZERO, STALE_AFTER);
            assert!(guard.0.is_none(), "a held lock must not be taken");
        }
        assert!(
            dir.is_dir(),
            "a guard that never took the lock must not release it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_caller_that_already_holds_the_lock_is_not_made_to_wait_for_itself() {
        let dir = scratch("reentrant");
        std::fs::create_dir_all(&dir).unwrap();
        // `hold()` reads the real path, so prove the env check short-circuits
        // before any filesystem work by making the real lock unavailable.
        let real = lock_dir();
        let taken = std::fs::create_dir(&real).is_ok();
        unsafe { std::env::set_var(ALREADY_HELD, "1") };
        let guard = hold();
        assert!(
            guard.0.is_none(),
            "a caller holding the lock must get a guard that releases nothing"
        );
        unsafe { std::env::remove_var(ALREADY_HELD) };
        drop(guard);
        if taken {
            assert!(real.is_dir(), "the caller's own lock must survive");
            let _ = std::fs::remove_dir(&real);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lock_left_by_a_crashed_run_is_broken() {
        let dir = scratch("stale");
        std::fs::create_dir_all(&dir).unwrap();
        {
            // Everything is older than a zero-length staleness window, which
            // is the same branch a 30-second-old lock takes.
            let guard = hold_in(dir.clone(), Duration::ZERO, Duration::ZERO);
            assert!(guard.0.is_some(), "a stale lock must be broken and taken");
        }
        assert!(!dir.exists());
    }
}
