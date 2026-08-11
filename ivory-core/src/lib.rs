pub mod detector;
pub mod overrides;
pub mod patterns;

pub use detector::ChordDetector;
#[cfg(feature = "learning")]
pub use detector::TrainOutcome;
pub use overrides::{OverrideInfo, OverrideStore};
