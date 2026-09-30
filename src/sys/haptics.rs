use std::sync::OnceLock;

use multitouch::{Actuator, Device, FeedbackPattern};

use crate::common::config::HapticPattern;

static ACTUATORS: OnceLock<Vec<Actuator>> = OnceLock::new();

fn actuators() -> &'static [Actuator] {
    ACTUATORS.get_or_init(|| {
        Device::all()
            .into_iter()
            .filter_map(|device| device.actuator())
            .filter(|actuator| actuator.open())
            .collect()
    })
}

#[inline]
fn pattern_index(pattern: HapticPattern) -> FeedbackPattern {
    match pattern {
        HapticPattern::Generic => FeedbackPattern::Firm,
        HapticPattern::Alignment => FeedbackPattern::FirmStrong,
        HapticPattern::LevelChange => FeedbackPattern::Medium,
    }
}

pub fn perform_haptic(pattern: HapticPattern) -> bool {
    actuators().iter().any(|actuator| actuator.actuate(pattern_index(pattern), 1.0))
}
