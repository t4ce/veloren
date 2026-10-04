// Scheduling boundaries for the server's synchronous world bootstrap.
// This is a worldgen policy, not a replacement thread scheduler.

pub(crate) struct WorkBudget {
    #[cfg(any(target_os = "trueos", feature = "cooperative-worldgen"))]
    last_turn: std::time::Instant,
}

impl WorkBudget {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(any(target_os = "trueos", feature = "cooperative-worldgen"))]
            last_turn: std::time::Instant::now(),
        }
    }

    /// Call between complete units of generation work, with no shared lock held.
    /// Logging is deliberately independent of these scheduling points.
    #[inline]
    pub(crate) fn checkpoint(&mut self) {
        #[cfg(any(target_os = "trueos", feature = "cooperative-worldgen"))]
        if self.last_turn.elapsed() >= std::time::Duration::from_millis(10) {
            std::thread::yield_now();
            self.last_turn = std::time::Instant::now();
        }
    }
}

/// Finish a mutable site's update before visiting the next site. The caller
/// retains the phase barrier: trade runs only after all updates return.
#[cfg(any(target_os = "trueos", feature = "cooperative-worldgen"))]
pub(crate) fn for_each_site<T>(sites: impl IntoIterator<Item = T>, mut update: impl FnMut(T)) {
    let mut budget = WorkBudget::new();
    for site in sites {
        update(site);
        budget.checkpoint();
    }
}

/// Retain every result (including absent samples) in input order, without a
/// parallel completion barrier during cooperative bootstrap.
#[cfg(any(target_os = "trueos", feature = "cooperative-worldgen"))]
pub(crate) fn collect_ordered<T, U>(items: impl IntoIterator<Item = T>, mut sample: impl FnMut(T) -> U) -> Vec<U> {
    let mut budget = WorkBudget::new();
    items.into_iter().map(|item| {
        let value = sample(item);
        budget.checkpoint();
        value
    }).collect()
}
