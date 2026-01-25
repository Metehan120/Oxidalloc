pub mod gtrim;
pub mod thread;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeDecay {
    Normal,
    Medium,
    High,
    Aggressive,
}

impl TimeDecay {
    pub fn decide_on(avg_time: usize) -> TimeDecay {
        match avg_time {
            0..=1000 => TimeDecay::Normal,
            1001..=2000 => TimeDecay::Medium,
            2001..=2500 => TimeDecay::High,
            _ => TimeDecay::Aggressive,
        }
    }

    pub fn get_trim_time(&self) -> u64 {
        match self {
            TimeDecay::Normal => 150,
            TimeDecay::Medium => 120,
            TimeDecay::High => 100,
            TimeDecay::Aggressive => 20,
        }
    }

    pub fn get_threshold(&self) -> u64 {
        match self {
            TimeDecay::Normal => 32 * 1024 * 1024,
            TimeDecay::Medium => 64 * 1024 * 1024,
            TimeDecay::High => 128 * 1024 * 1024,
            TimeDecay::Aggressive => 512 * 1024 * 1024,
        }
    }

    pub fn from_u8(value: u8) -> TimeDecay {
        match value {
            0 => TimeDecay::Normal,
            1 => TimeDecay::Medium,
            2 => TimeDecay::High,
            3 => TimeDecay::Aggressive,
            _ => TimeDecay::Aggressive,
        }
    }
}
