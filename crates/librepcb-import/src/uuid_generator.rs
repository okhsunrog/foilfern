//! Source of the UUIDs of imported objects (upstream
//! `std::function<Uuid()> createUuid`).

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use librepcb_core::types::Uuid;

/// Creates the UUIDs of imported objects: random by default, or sequential
/// for reproducible output (e.g. in tests).
#[derive(Clone)]
pub struct UuidGenerator(Arc<dyn Fn() -> Uuid + Send + Sync>);

impl UuidGenerator {
    /// Random (version 4) UUIDs, like upstream `Uuid::createRandom()`.
    pub fn random() -> Self {
        Self(Arc::new(Uuid::new_random))
    }

    /// Sequential UUIDs `00000000-0000-4000-8000-000000000001`, `...0002`,
    /// ... (each generator starts at 1).
    pub fn sequential() -> Self {
        let counter = AtomicU64::new(0);
        Self(Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
            format!("00000000-0000-4000-8000-{n:012x}")
                .parse()
                .expect("formatted string is a valid UUID")
        }))
    }

    /// UUIDs created by a custom function.
    pub fn from_fn(f: impl Fn() -> Uuid + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Returns a new UUID.
    pub fn generate(&self) -> Uuid {
        (self.0)()
    }
}

impl Default for UuidGenerator {
    fn default() -> Self {
        Self::random()
    }
}

impl fmt::Debug for UuidGenerator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UuidGenerator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequential() {
        let g = UuidGenerator::sequential();
        assert_eq!(
            g.generate().to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            g.generate().to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert_ne!(UuidGenerator::random().generate(), g.generate());
    }
}
