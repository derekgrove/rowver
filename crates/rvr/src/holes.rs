//! What a press does when it cannot read something: a file, or a run of its entries, becomes a
//! hole of one kind (docs/FORMAT.md, "Holes"), or the press ends. A mistake in the study or the
//! recipe ends it; so does a failure that is not the file's own (a proxy or certificate refused, a
//! local file that cannot be read for want of rights), which would fail every later file the same,
//! and so makes no holes.

use std::sync::Mutex;

/// Why an input, or a run of its entries, was not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Every copy of a dataset's file is on tape only.
    Tape,
    /// Its storage kept failing after every wait.
    Unreachable,
    /// Its bytes do not decompress, or are not ROOT's.
    Damaged,
    /// It is not there any more.
    Gone,
    /// It is not the file the lock names.
    Changed,
    /// Its storage refuses it with a valid proxy.
    Refused,
    /// Not the file's fault, and every later file would fail the same: the press stops, and
    /// makes no hole of it.
    Save,
    /// A mistake in the study or the recipe: the press stops, and writes nothing.
    Mistake,
    /// Another input ended the press.
    Stopped,
    /// A bug in rvr: the press stops, and keeps what it read.
    Bug,
}

impl Fault {
    /// The kind of hole it leaves, or `None` when it ends the press.
    pub fn kind(self) -> Option<&'static str> {
        Some(match self {
            Fault::Tape => "tape",
            Fault::Unreachable => "unreachable",
            Fault::Damaged => "damaged",
            Fault::Gone => "gone",
            Fault::Changed => "changed",
            Fault::Refused => "refused",
            Fault::Save | Fault::Mistake | Fault::Stopped | Fault::Bug => return None,
        })
    }

    /// Whether the entries after it in the same file can still be read: only a damaged basket
    /// leaves the rest of its file readable.
    pub fn local(self) -> bool {
        self == Fault::Damaged
    }
}

/// Whether a hole of `kind` may pass (a storage busy, down or refusing; tape), so is read again.
pub fn passing(kind: &str) -> bool {
    matches!(kind, "unreachable" | "refused" | "tape")
}

/// A failure to read, what it is and why, in words for a person.
#[derive(Clone, Debug)]
pub struct Failed {
    pub fault: Fault,
    pub why: String,
}

impl Failed {
    /// A failure of this kind, and why.
    pub fn new(fault: Fault, why: impl Into<String>) -> Self {
        Failed { fault, why: why.into() }
    }
}

/// A mistake (the default for what is not known to be a read failure).
impl From<String> for Failed {
    fn from(why: String) -> Self {
        Failed::new(Fault::Mistake, why)
    }
}

/// What the last request to a remote file that failed means for the file.
#[derive(Default)]
pub struct Seen(Mutex<Option<Fault>>);

impl Seen {
    /// Note a failed request.
    pub fn note(&self, fault: Fault) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(fault);
    }

    /// A request succeeded: what failed before it says nothing of what fails next.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The last failed request's fault, if one failed since the last that succeeded.
    pub fn last(&self) -> Option<Fault> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What a hep-root error reading a file means for it (for a remote file, as `seen` says).
// A branch of a type hep-root cannot read, or a name or entry it does not have, is the recipe's
// or rvr's mistake, not the file's.
pub fn fault_of(error: &hep_root::Error, seen: Option<&Seen>) -> Fault {
    use hep_root::Error as E;
    let remote = seen.and_then(Seen::last);
    match error {
        E::Remote(why) if why.contains(crate::samples::remote::STOPPED) => Fault::Stopped,
        // A remote file answers 404 as a changed source; its request said it is gone.
        E::Remote(_) | E::Io(_) | E::SourceChanged(_) if remote.is_some() => remote.unwrap_or(Fault::Unreachable),
        E::Remote(_) => Fault::Unreachable,
        E::SourceChanged(_) => Fault::Changed,
        // A local file not there is gone, one cut short is damaged; one that cannot be read (no
        // rights, a mount gone, too many open files) is not the file's fault.
        E::Io(e) if e.kind() == std::io::ErrorKind::NotFound => Fault::Gone,
        E::Io(e) if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::InvalidData) => Fault::Damaged,
        E::Io(_) => Fault::Save,
        E::Format { .. } | E::Compression { .. } | E::NoBasket { .. } => Fault::Damaged,
        E::Unsupported(_) | E::EntryOutOfRange { .. } | E::BranchNotFound(_) | E::KeyNotFound(_) => Fault::Mistake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_failure_is_named_by_what_it_is() {
        let compression = hep_root::Error::Compression { algorithm: "zlib".into(), message: "bad".into() };
        assert_eq!(fault_of(&compression, None), Fault::Damaged);
        let busy = hep_root::Error::Remote("f.root: its storage answered 503".into());
        assert_eq!(fault_of(&busy, None), Fault::Unreachable);
        let seen = Seen::default();
        seen.note(Fault::Refused);
        assert_eq!(fault_of(&busy, Some(&seen)), Fault::Refused);
        let stopped = hep_root::Error::Remote(crate::samples::remote::STOPPED.into());
        assert_eq!(fault_of(&stopped, Some(&seen)), Fault::Stopped);
        assert_eq!(fault_of(&hep_root::Error::Unsupported("TObjArray".into()), None), Fault::Mistake);
        seen.note(Fault::Gone);
        let deleted = hep_root::Error::SourceChanged("f.root no longer exists".into());
        assert_eq!(fault_of(&deleted, Some(&seen)), Fault::Gone);
        assert_eq!(fault_of(&deleted, None), Fault::Changed);
    }

    #[test]
    fn every_kind_of_hole_is_one_the_reader_knows() {
        use Fault::*;
        let kinds: Vec<&str> = [Tape, Unreachable, Damaged, Gone, Changed, Refused, Save, Mistake, Stopped, Bug]
            .iter()
            .filter_map(|fault| fault.kind())
            .collect();
        assert_eq!(kinds, rowver::rvr::HOLE_KINDS);
        // Only what is about the file's bytes or place lasts.
        let lasting: Vec<&str> = kinds.into_iter().filter(|kind| !passing(kind)).collect();
        assert_eq!(lasting, ["damaged", "gone", "changed"]);
    }
}
