//! Optional memoization of pure pass work. The program supplies storage; core knows only keys and bytes.
use std::sync::Arc;

/// A content digest in an operation's versioned namespace. The producer owns the hashing contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(pub [u8; 32]);

/// Advisory storage for deterministic computations. A missing value or failed write costs recomputation only.
/// Implementations verify stored bytes and account for storage failures; passes report hits and misses.
pub trait Store: std::fmt::Debug + Send + Sync {
    fn get(&self, key: &Key) -> Option<Vec<u8>>;
    fn put(&self, key: &Key, value: &[u8]);
}

/// The run's caller-provided memo store (`core.pass_memo`). Absent: passes compute every result.
/// Cloned worlds share this service, as they share other caller-provided execution services.
///
/// `build` is the program's statement of which code computed the entries: the primase revision it was built against.
/// Passes mix it into every key, so a build with different code never reads another build's entries even when the
/// pass's own version, options and inputs are unchanged. The library cannot know its revision; the program does.
#[derive(Clone, Debug)]
pub struct Memo {
    pub store: Arc<dyn Store>,
    pub build: String,
}

impl Memo {
    /// A store and the build identity its entries are valid for. `build` must not be empty.
    pub fn new(store: Arc<dyn Store>, build: impl Into<String>) -> Result<Self, String> {
        let build = build.into();
        if build.is_empty() {
            return Err(
                "a memo store needs the build identity (the primase revision) its entries are valid for"
                    .into(),
            );
        }
        Ok(Self { store, build })
    }
}

impl crate::Resource for Memo {
    const NAME: &'static str = "core.pass_memo";
}
